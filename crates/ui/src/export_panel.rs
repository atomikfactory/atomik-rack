//! The audio export panel: the options of File > Export Song / Export Loop as Audio File and
//! Export Stems, and the progress of the render that follows.
//!
//! ```text
//! EXPORT SONG AS AUDIO FILE
//! my song.wav
//!   BIT DEPTH   [ 16-BIT ] [ 24-BIT ] [ 32-BIT FLOAT ]
//!   DITHER      [ NONE ] [ TPDF ] [ SHAPED ]
//!   SAMPLE RATE [ PROJECT ] [ 44.1 kHz ] [ 48 kHz ] [ 88.2 kHz ] [ 96 kHz ]
//!   FILE FORMAT [ WAV ] [ AIFF ] [ FLAC ] [ MP3 ] [ AAC ] [ OPUS ]
//!   QUALITY     [ 128 kbps ] [ 192 kbps ] [ 256 kbps ] [ 320 kbps ] [ V0 ] [ V2 ] [ V4 ]
//!   TAIL        [ KEEP EFFECT TAIL ]
//!   Renders to the End marker, then keeps the tail while it is audible.
//!                                                          [ EXPORT ] [ CANCEL ]
//! ```
//!
//! A modal view the app places over the window like the relink panel. It has two modes. In the
//! **options** mode ([`ExportView::open`]) the buttons change the [`ExportValues`] the view
//! holds; EXPORT emits [`ExportAction::Start`] with them and CANCEL (or Esc) emits
//! [`ExportAction::Cancel`]. The dither applies to 16-bit files only, so its row is greyed at the
//! other depths; the tail applies to a song and to stems (which also have a CHANNELS row: after the
//! strip or dry, and a RANGE row: the whole song, only what lies between the locators, or the time
//! range selected on the lanes). MP3, AAC and Opus are encoded by the user's FFmpeg: the app says
//! through [`ExportView::set_encoder`] whether it can be used, and without it the three buttons
//! are greyed with a line that says how to set FFmpeg up (stems are always WAV, AIFF or FLAC). A
//! compressed file has no bit depth or dither, so those rows grey out, and its QUALITY row
//! offers the bitrates of the format (MP3 also three variable-bitrate qualities). FLAC
//! holds 16-bit and 24-bit audio, so the 32-bit float button is inert while FLAC is chosen (and
//! choosing FLAC at 32-bit float moves the depth to 24-bit). In the **progress** mode
//! ([`ExportView::show_progress`]) a bar shows the fraction the app feeds with
//! [`ExportView::set_progress`], and CANCEL (or Esc) emits [`ExportAction::Cancel`] to stop the
//! render. The view never closes itself: the app drops it when the export is over.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::pick_list::{self, PickList};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect, RoundedRect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const FILE_H: f64 = 26.0;
const ROW_H: f64 = 32.0;
const HINT_H: f64 = 30.0;
const FOOTER_H: f64 = 44.0;
const BAR_H: f64 = 14.0;
const PROGRESS_H: f64 = 74.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 540.0;
const BTN_H: f64 = 22.0;
const BTN_GAP: f64 = 6.0;
const LABEL_W: f64 = 104.0;
/// The width of a button of the file format row (six of them share the row).
const FILE_BTN_W: f64 = 62.0;

/// The width of a quality button: a bitrate is wider than a VBR quality.
fn quality_width(quality: ExportQuality) -> f64 {
    match quality {
        ExportQuality::Cbr(_) => 58.0,
        ExportQuality::Vbr(_) => 44.0,
    }
}

/// What is exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    /// From the start to the End marker, plus the tail.
    Song,
    /// From the left to the right locator.
    Loop,
    /// The time range selected on the lanes of the arrangement, exactly, with no tail.
    Selection,
    /// The song as one file per mixer strip (channels, buses, returns and the master), in a
    /// folder.
    Stems,
}

/// The container of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFile {
    /// A WAV file.
    Wav,
    /// An AIFF file.
    Aiff,
    /// A FLAC file (16-bit and 24-bit only).
    Flac,
    /// An MP3 file (encoded by the user's FFmpeg).
    Mp3,
    /// An AAC file in an `.m4a` container (encoded by the user's FFmpeg).
    Aac,
    /// An Opus file (encoded by the user's FFmpeg).
    Opus,
}

impl ExportFile {
    /// True for the formats that FFmpeg encodes (they have a quality, no bit depth or dither).
    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Mp3 | Self::Aac | Self::Opus)
    }
}

/// How hard a compressed format squeezes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportQuality {
    /// A constant bitrate in kilobits per second.
    Cbr(u16),
    /// A variable bitrate by quality, `0` (best) to `9`; MP3 only.
    Vbr(u8),
}

/// The constant bitrates the panel offers for MP3, in kilobits per second.
pub const MP3_BITRATES: [u16; 4] = [128, 192, 256, 320];
/// The variable-bitrate qualities the panel offers for MP3 (`0` is the best).
pub const MP3_VBR: [u8; 3] = [0, 2, 4];
/// The bitrates the panel offers for AAC, in kilobits per second.
pub const AAC_BITRATES: [u16; 4] = [128, 192, 256, 320];
/// The bitrates the panel offers for Opus, in kilobits per second.
pub const OPUS_BITRATES: [u16; 5] = [64, 96, 128, 160, 192];

/// Whether FFmpeg, which encodes the compressed formats, can be used now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EncoderState {
    /// FFmpeg is found.
    #[default]
    Ready,
    /// FFmpeg is being looked for.
    Checking,
    /// FFmpeg cannot be used; the text says how to set it up.
    Missing(String),
}

/// The bit depth of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDepth {
    /// 16-bit PCM.
    Pcm16,
    /// 24-bit PCM.
    Pcm24,
    /// 32-bit float.
    Float32,
}

/// The dither of a 16-bit file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDither {
    /// None: plain rounding.
    Off,
    /// Triangular dither.
    Tpdf,
    /// Triangular dither with the noise shaped towards the high frequencies.
    Shaped,
}

/// The sample rates the panel offers besides the project's, in Hz.
pub const EXPORT_RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

/// What an export does about loudness. A whole number of LUFS is all the presets and the panel
/// need; the true-peak ceiling of a target is [`ExportLoudness::CEILING_DBTP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportLoudness {
    /// Leave the level as the song has it.
    #[default]
    Off,
    /// Bring the integrated loudness to this many LUFS with the true peak under the ceiling
    /// (limiting if the gain needs it).
    Target(i8),
    /// Only a guide: measure, report the distance and warn when the peaks clip; change nothing.
    Guide(i8),
}

impl ExportLoudness {
    /// The true-peak ceiling of a loudness target, in dBTP.
    pub const CEILING_DBTP: f32 = -1.0;

    /// The choices the panel offers, in order.
    pub const CHOICES: [Self; 4] = [
        Self::Off,
        Self::Target(-16),
        Self::Target(-14),
        Self::Guide(-9),
    ];

    /// The button label: `OFF`, `-14 LUFS`, `-9 GUIDE`.
    pub fn label(self) -> String {
        match self {
            Self::Off => "OFF".to_owned(),
            Self::Target(l) => format!("{l} LUFS"),
            Self::Guide(l) => format!("{l} GUIDE"),
        }
    }
}

/// What the panel sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportValues {
    /// The bit depth.
    pub depth: ExportDepth,
    /// The dither (used at 16-bit only).
    pub dither: ExportDither,
    /// The sample rate in Hz; `None` is the project's.
    pub rate: Option<u32>,
    /// Keep the tail of the song after the End while it is audible.
    pub tail: bool,
    /// The container of the file.
    pub file: ExportFile,
    /// Stems only: take the channels before their strips (the source's own sound) instead of
    /// after them.
    pub dry: bool,
    /// Stems only: export just the range between the locators (no tail) instead of the song.
    pub locators: bool,
    /// Stems only: export just the time range selected on the lanes (no tail). It wins over
    /// `locators`. It is not remembered between sessions.
    pub selection: bool,
    /// The quality of an MP3 file.
    pub mp3: ExportQuality,
    /// The bitrate of an AAC file in kilobits per second.
    pub aac: u16,
    /// The bitrate of an Opus file in kilobits per second.
    pub opus: u16,
    /// Mix every channel into one before writing.
    pub mono: bool,
    /// What to do about the loudness of the mix (not for stems).
    pub loudness: ExportLoudness,
    /// Also write an MP3 at [`ALSO_MP3_KBPS`] next to a lossless file, from the same render (not
    /// for stems).
    pub also_mp3: bool,
}

/// The bitrate of the extra MP3 file, in kilobits per second.
pub const ALSO_MP3_KBPS: u16 = 320;

impl ExportValues {
    /// The quality of the compressed format chosen, `None` for a lossless one.
    pub fn quality(&self) -> Option<ExportQuality> {
        match self.file {
            ExportFile::Mp3 => Some(self.mp3),
            ExportFile::Aac => Some(ExportQuality::Cbr(self.aac)),
            ExportFile::Opus => Some(ExportQuality::Cbr(self.opus)),
            _ => None,
        }
    }
}

impl Default for ExportValues {
    fn default() -> Self {
        Self {
            depth: ExportDepth::Pcm24,
            dither: ExportDither::Tpdf,
            rate: None,
            tail: true,
            file: ExportFile::Wav,
            dry: false,
            locators: false,
            selection: false,
            mp3: ExportQuality::Cbr(192),
            aac: 192,
            opus: 128,
            mono: false,
            loudness: ExportLoudness::Off,
            also_mp3: false,
        }
    }
}

/// A preset the panel can fill its fields from: what the app lists in the Preset row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetChoice {
    /// The name shown in the list.
    pub name: String,
    /// The preset exports stems (one file per strip) instead of the mix.
    pub stems: bool,
    /// The panel's fields.
    pub values: ExportValues,
    /// A preset of the user's (it can be deleted), not a built-in one.
    pub user: bool,
}

impl PresetChoice {
    /// Whether the preset writes a compressed file, which FFmpeg encodes.
    pub fn needs_ffmpeg(&self) -> bool {
        !self.stems && (self.values.file.is_lossy() || self.values.also_mp3)
    }
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportAction {
    /// Start the export with these values.
    Start(ExportValues),
    /// Close the options, or stop the render that runs.
    Cancel,
    /// Keep the panel's settings as a preset under the name typed
    /// ([`ExportView::preset_name_entry`]).
    SavePreset,
    /// Delete the user preset the panel shows ([`ExportView::preset`]).
    DeletePreset,
}

/// The range of a stems export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeChoice {
    Song,
    Locators,
    Selection,
}

/// A button of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Depth(ExportDepth),
    Dither(ExportDither),
    Rate(Option<u32>),
    File(ExportFile),
    Quality(ExportQuality),
    Tail,
    Dry(bool),
    Range(RangeChoice),
    Mono(bool),
    Loudness(ExportLoudness),
    Extra(bool),
    PresetList,
    PresetSave,
    PresetDelete,
    NameOk,
    NameCancel,
    Start,
    Cancel,
}

/// The panel mode.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Options,
    Progress { fraction: f32, text: String },
}

/// The export panel. See the module documentation.
#[derive(Debug)]
pub struct ExportView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    scope: ExportScope,
    file: String,
    project_rate: u32,
    values: ExportValues,
    encoder: EncoderState,
    warning: Option<String>,
    mode: Mode,
    hover: Option<Point>,
    repaint: bool,
    presets: Vec<PresetChoice>,
    /// The preset the fields were last filled from, and what it said.
    preset: Option<String>,
    base: (bool, ExportValues),
    /// The scope the panel was opened with (a mix preset brings a stems panel back to it).
    home: ExportScope,
    list: Option<PickList>,
    naming: Option<TextField>,
    selecting: bool,
}

impl ExportView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A panel in the options mode for a song; call [`Self::open`] to set it up.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            scope: ExportScope::Song,
            file: String::new(),
            project_rate: 48_000,
            values: ExportValues::default(),
            encoder: EncoderState::Ready,
            warning: None,
            mode: Mode::Options,
            hover: None,
            repaint: true,
            presets: Vec::new(),
            preset: None,
            base: (false, ExportValues::default()),
            home: ExportScope::Song,
            list: None,
            naming: None,
            selecting: false,
        }
    }

    /// Places the panel: the area it may use (logical pixels). It is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Shows the options for exporting `scope` to the file called `file`, with the last `values`;
    /// `project_rate` is the project's sample rate in Hz (named on its button).
    pub fn open(
        &mut self,
        scope: ExportScope,
        file: &str,
        project_rate: u32,
        values: ExportValues,
    ) {
        self.scope = scope;
        self.home = if scope == ExportScope::Stems {
            ExportScope::Song
        } else {
            scope
        };
        self.file = file.to_owned();
        self.project_rate = project_rate;
        self.values = values;
        self.warning = None;
        self.mode = Mode::Options;
        self.preset = None;
        self.list = None;
        self.naming = None;
        self.settle_file();
        self.repaint = true;
    }

    /// Shows a line under the options (the app says why it did not start, or what to confirm: a
    /// file that the export would replace); it goes away when a button changes a choice.
    pub fn set_warning(&mut self, text: Option<String>) {
        if text != self.warning {
            self.warning = text;
            self.repaint = true;
        }
    }

    /// The line shown under the options, if any.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// Tells the panel whether FFmpeg can encode the compressed formats. Without it their
    /// buttons are greyed and a line says how to set FFmpeg up; a compressed format chosen
    /// before that was known is dropped for WAV when FFmpeg turns out to be missing.
    pub fn set_encoder(&mut self, state: EncoderState) {
        if state != self.encoder {
            self.encoder = state;
            self.settle_file();
            self.repaint = true;
        }
    }

    /// The compressed formats cannot be chosen for stems or without FFmpeg: the choice falls back
    /// to WAV.
    fn settle_file(&mut self) {
        let unusable =
            self.scope == ExportScope::Stems || matches!(self.encoder, EncoderState::Missing(_));
        if self.values.file.is_lossy() && unusable {
            self.values.file = ExportFile::Wav;
        }
    }

    /// Whether the compressed formats can be pressed now.
    fn lossy_enabled(&self) -> bool {
        self.scope != ExportScope::Stems && !matches!(self.encoder, EncoderState::Missing(_))
    }

    /// Switches to the progress bar (the render has started).
    pub fn show_progress(&mut self) {
        self.mode = Mode::Progress {
            fraction: 0.0,
            text: String::new(),
        };
        self.repaint = true;
    }

    /// Sets the fraction done, `0.0..=1.0`, and the text beside the bar. Cheap when nothing
    /// changed.
    pub fn set_progress(&mut self, fraction: f32, text: &str) {
        let fraction = fraction.clamp(0.0, 1.0);
        if let Mode::Progress {
            fraction: f,
            text: t,
        } = &mut self.mode
            && (f.to_bits() != fraction.to_bits() || t != text)
        {
            *f = fraction;
            text.clone_into(t);
            self.repaint = true;
        }
    }

    /// True while the progress bar is shown.
    pub fn in_progress(&self) -> bool {
        matches!(self.mode, Mode::Progress { .. })
    }

    /// The values the buttons have set.
    pub fn values(&self) -> ExportValues {
        self.values
    }

    /// What is being exported.
    pub fn scope(&self) -> ExportScope {
        self.scope
    }

    // ------------------------------------------------------------------ presets

    /// Sets the presets the Preset row lists (the built-in ones, then the user's). The preset
    /// shown stays when it is still in the list.
    pub fn set_presets(&mut self, presets: Vec<PresetChoice>) {
        self.presets = presets;
        if self
            .preset
            .as_ref()
            .is_some_and(|name| !self.presets.iter().any(|p| &p.name == name))
        {
            self.preset = None;
        }
        self.list = None;
        self.repaint = true;
    }

    /// The presets the Preset row lists.
    pub fn presets(&self) -> &[PresetChoice] {
        &self.presets
    }

    /// The name of the preset the fields were filled from, if any (see [`Self::preset_changed`]).
    pub fn preset(&self) -> Option<&str> {
        self.preset.as_deref()
    }

    /// Whether the fields differ from what the preset shown said.
    pub fn preset_changed(&self) -> bool {
        self.preset.is_some() && (self.scope == ExportScope::Stems, self.values) != self.base
    }

    /// The text of the Preset row's drop-down: the name, with `(changed)` after it when a field
    /// was touched since; `(NONE)` when no preset was chosen.
    pub fn preset_label(&self) -> String {
        match &self.preset {
            None => "(NONE)".to_owned(),
            Some(name) if self.preset_changed() => format!("{name} (changed)"),
            Some(name) => name.clone(),
        }
    }

    /// Whether the preset shown is one of the user's (so it can be deleted).
    pub fn preset_is_user(&self) -> bool {
        self.preset
            .as_ref()
            .is_some_and(|n| self.presets.iter().any(|p| &p.name == n && p.user))
    }

    /// Whether a preset can be chosen now: one that writes a compressed file needs FFmpeg.
    pub fn preset_usable(&self, preset: &PresetChoice) -> bool {
        !(preset.needs_ffmpeg() && matches!(self.encoder, EncoderState::Missing(_)))
    }

    /// Fills the fields from the preset called `name`, as choosing it in the list does. A preset
    /// of stems makes this a stems export, a preset of the mix brings a stems panel back to the
    /// scope it was opened with. Returns false for a name that is not listed or a preset that
    /// cannot be used (it needs FFmpeg).
    pub fn choose_preset(&mut self, name: &str) -> bool {
        let Some(preset) = self.presets.iter().find(|p| p.name == name).cloned() else {
            return false;
        };
        if !self.preset_usable(&preset) || self.in_progress() {
            return false;
        }
        self.values = preset.values;
        self.scope = if preset.stems {
            ExportScope::Stems
        } else if self.scope == ExportScope::Stems {
            self.home
        } else {
            self.scope
        };
        self.warning = None;
        self.list = None;
        self.settle_file();
        self.preset = Some(preset.name);
        self.base = (self.scope == ExportScope::Stems, self.values);
        self.repaint = true;
        true
    }

    /// The app kept the fields as the preset `name`: the panel shows it, unchanged, and leaves
    /// the name entry.
    pub fn preset_saved(&mut self, name: &str) {
        self.preset = Some(name.to_owned());
        self.base = (self.scope == ExportScope::Stems, self.values);
        self.naming = None;
        self.repaint = true;
    }

    /// Whether the name of a new preset is being typed.
    pub fn is_naming(&self) -> bool {
        self.naming.is_some()
    }

    /// The name typed so far, while one is being typed.
    pub fn preset_name_entry(&self) -> Option<&str> {
        self.naming.as_ref().map(TextField::text)
    }

    /// Whether a text field has the keys (the name of a preset).
    pub fn wants_text(&self) -> bool {
        self.naming.is_some()
    }

    /// Whether the preset list is open.
    pub fn list_open(&self) -> bool {
        self.list.is_some()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ geometry

    /// The rows of the options, in order: the preset, then bit depth, dither, sample rate, file
    /// format and quality for every scope, the tail (songs and stems), then the strip and the
    /// range (stems) or the mix-down, the loudness and the extra file (the rest).
    fn row_names(&self) -> Vec<&'static str> {
        let mut rows = vec![
            "PRESET",
            "BIT DEPTH",
            "DITHER",
            "SAMPLE RATE",
            "FILE FORMAT",
            "QUALITY",
        ];
        if !matches!(self.scope, ExportScope::Loop | ExportScope::Selection) {
            rows.push("TAIL");
        }
        if self.scope == ExportScope::Stems {
            rows.push("CHANNELS");
            rows.push("RANGE");
        } else {
            rows.push("MIX DOWN");
            rows.push("LOUDNESS");
            rows.push("EXTRA FILE");
        }
        rows
    }

    fn rows(&self) -> usize {
        self.row_names().len()
    }

    /// The index of the row called `name`.
    fn row_of(&self, name: &str) -> usize {
        self.row_names()
            .iter()
            .position(|n| *n == name)
            .unwrap_or_default()
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let height = match self.mode {
            Mode::Options => HEADER_H + FILE_H + self.rows() as f64 * ROW_H + HINT_H + FOOTER_H,
            Mode::Progress { .. } => HEADER_H + PROGRESS_H + FOOTER_H,
        };
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + height)
    }

    fn row_top(&self, row: usize) -> f64 {
        self.panel().y0 + HEADER_H + FILE_H + row as f64 * ROW_H
    }

    fn button(&self, row: usize, index: usize, width: f64) -> Rect {
        self.button_at(row, index as f64 * (width + BTN_GAP), width)
    }

    /// The button of `row` that starts `offset` to the right of the first one.
    fn button_at(&self, row: usize, offset: f64, width: f64) -> Rect {
        let p = self.panel();
        let x = p.x0 + PAD + LABEL_W + offset;
        let y = self.row_top(row) + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + width, y + BTN_H)
    }

    /// The quality buttons of the chosen compressed format with their labels; none for a lossless
    /// format.
    fn quality_choices(&self) -> Vec<(ExportQuality, String)> {
        let cbr = |rates: &[u16]| -> Vec<(ExportQuality, String)> {
            rates
                .iter()
                .map(|&k| (ExportQuality::Cbr(k), format!("{k} kbps")))
                .collect()
        };
        match self.values.file {
            ExportFile::Mp3 => {
                let mut all = cbr(&MP3_BITRATES);
                all.extend(
                    MP3_VBR
                        .iter()
                        .map(|&q| (ExportQuality::Vbr(q), format!("V{q}"))),
                );
                all
            }
            ExportFile::Aac => cbr(&AAC_BITRATES),
            ExportFile::Opus => cbr(&OPUS_BITRATES),
            _ => Vec::new(),
        }
    }

    /// How far right of the first button the quality button `index` starts.
    fn quality_offset(&self, index: usize) -> f64 {
        self.quality_choices()
            .iter()
            .take(index)
            .map(|(q, _)| quality_width(*q) + BTN_GAP)
            .sum()
    }

    /// The preset drop-down button.
    fn preset_rect(&self) -> Rect {
        self.button_at(0, 0.0, 180.0)
    }

    /// Where the open preset list goes: under the drop-down.
    fn list_at(&self) -> PickList {
        let r = self.preset_rect();
        let mut list = PickList::new(
            Point::new(r.x0, r.y1 + 2.0),
            240.0,
            10,
            self.presets.len(),
            0,
        );
        if let Some(name) = &self.preset
            && let Some(i) = self.presets.iter().position(|p| &p.name == name)
        {
            list.top = (i + 1)
                .saturating_sub(list.max_rows)
                .min(list.len.saturating_sub(list.max_rows));
        }
        list
    }

    /// The name field of a new preset.
    fn name_rect(&self) -> Rect {
        self.button_at(0, 0.0, 230.0)
    }

    fn footer_button(&self, from_right: usize) -> Rect {
        let p = self.panel();
        let x1 = p.x1 - PAD - from_right as f64 * (96.0 + BTN_GAP);
        Rect::new(
            x1 - 96.0,
            p.y1 - FOOTER_H + (FOOTER_H - BTN_H) / 2.0,
            x1,
            p.y1 - FOOTER_H + (FOOTER_H - BTN_H) / 2.0 + BTN_H,
        )
    }

    /// The progress bar.
    pub fn bar_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + 34.0;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + BAR_H)
    }

    /// Every button with its label, whether it is lit and whether it can be pressed.
    fn buttons(&self) -> Vec<(Hit, Rect, String, bool, bool)> {
        let v = self.values;
        let mut out = Vec::new();
        if self.in_progress() {
            out.push((
                Hit::Cancel,
                self.footer_button(0),
                "CANCEL".to_owned(),
                false,
                true,
            ));
            return out;
        }
        let (depth_row, dither_row, rate_row, file_row, quality_row) = (
            self.row_of("BIT DEPTH"),
            self.row_of("DITHER"),
            self.row_of("SAMPLE RATE"),
            self.row_of("FILE FORMAT"),
            self.row_of("QUALITY"),
        );
        if self.naming.is_some() {
            out.push((
                Hit::NameOk,
                self.button_at(0, 236.0, 60.0),
                "SAVE".to_owned(),
                false,
                self.name_ok(),
            ));
            out.push((
                Hit::NameCancel,
                self.button_at(0, 302.0, 70.0),
                "CANCEL".to_owned(),
                false,
                true,
            ));
        } else {
            out.push((
                Hit::PresetList,
                self.preset_rect(),
                fit_label(&self.preset_label(), 150.0, 9.5),
                false,
                !self.presets.is_empty(),
            ));
            out.push((
                Hit::PresetSave,
                self.button_at(0, 186.0, 110.0),
                "SAVE PRESET...".to_owned(),
                false,
                true,
            ));
            out.push((
                Hit::PresetDelete,
                self.button_at(0, 302.0, 100.0),
                "DELETE PRESET".to_owned(),
                false,
                self.preset_is_user(),
            ));
        }
        let w = 92.0;
        for (i, (depth, label)) in [
            (ExportDepth::Pcm16, "16-BIT"),
            (ExportDepth::Pcm24, "24-BIT"),
            (ExportDepth::Float32, "32-BIT FLOAT"),
        ]
        .into_iter()
        .enumerate()
        {
            // FLAC has no 32-bit float, and a compressed file has no bit depth.
            let lossy = v.file.is_lossy();
            let enabled = !lossy && !(v.file == ExportFile::Flac && depth == ExportDepth::Float32);
            out.push((
                Hit::Depth(depth),
                self.button(depth_row, i, w),
                label.to_owned(),
                !lossy && v.depth == depth,
                enabled,
            ));
        }
        let dither_on = v.depth == ExportDepth::Pcm16 && !v.file.is_lossy();
        for (i, (dither, label)) in [
            (ExportDither::Off, "NONE"),
            (ExportDither::Tpdf, "TPDF"),
            (ExportDither::Shaped, "SHAPED"),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::Dither(dither),
                self.button(dither_row, i, w),
                label.to_owned(),
                dither_on && v.dither == dither,
                dither_on,
            ));
        }
        let rw = 76.0;
        out.push((
            Hit::Rate(None),
            self.button(rate_row, 0, rw),
            "PROJECT".to_owned(),
            v.rate.is_none(),
            true,
        ));
        for (i, rate) in EXPORT_RATES.into_iter().enumerate() {
            out.push((
                Hit::Rate(Some(rate)),
                self.button(rate_row, i + 1, rw),
                rate_label(rate),
                v.rate == Some(rate),
                true,
            ));
        }
        for (i, (file, label)) in [
            (ExportFile::Wav, "WAV"),
            (ExportFile::Aiff, "AIFF"),
            (ExportFile::Flac, "FLAC"),
            (ExportFile::Mp3, "MP3"),
            (ExportFile::Aac, "AAC"),
            (ExportFile::Opus, "OPUS"),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::File(file),
                self.button(file_row, i, FILE_BTN_W),
                label.to_owned(),
                v.file == file,
                !file.is_lossy() || self.lossy_enabled(),
            ));
        }
        for (i, (quality, label)) in self.quality_choices().into_iter().enumerate() {
            let offset = self.quality_offset(i);
            out.push((
                Hit::Quality(quality),
                self.button_at(quality_row, offset, quality_width(quality)),
                label,
                v.quality() == Some(quality),
                true,
            ));
        }
        let tail_row = self.row_of("TAIL");
        if !matches!(self.scope, ExportScope::Loop | ExportScope::Selection) {
            out.push((
                Hit::Tail,
                self.button(tail_row, 0, 150.0),
                "KEEP EFFECT TAIL".to_owned(),
                v.tail,
                true,
            ));
        }
        if self.scope == ExportScope::Stems {
            for (i, (dry, label)) in [(false, "AFTER STRIP"), (true, "DRY SOURCE")]
                .into_iter()
                .enumerate()
            {
                out.push((
                    Hit::Dry(dry),
                    self.button(self.row_of("CHANNELS"), i, 110.0),
                    label.to_owned(),
                    v.dry == dry,
                    true,
                ));
            }
            let chosen = if v.selection {
                RangeChoice::Selection
            } else if v.locators {
                RangeChoice::Locators
            } else {
                RangeChoice::Song
            };
            for (choice, label, offset, width) in [
                (RangeChoice::Song, "WHOLE SONG", 0.0, 110.0),
                (RangeChoice::Locators, "BETWEEN LOCATORS", 116.0, 140.0),
                (RangeChoice::Selection, "SELECTION", 262.0, 100.0),
            ] {
                out.push((
                    Hit::Range(choice),
                    self.button_at(self.row_of("RANGE"), offset, width),
                    label.to_owned(),
                    chosen == choice,
                    true,
                ));
            }
        }
        if self.scope != ExportScope::Stems {
            let mix_row = self.row_of("MIX DOWN");
            for (i, (mono, label)) in [(false, "STEREO"), (true, "MONO")].into_iter().enumerate() {
                out.push((
                    Hit::Mono(mono),
                    self.button(mix_row, i, 80.0),
                    label.to_owned(),
                    v.mono == mono,
                    true,
                ));
            }
            let loud_row = self.row_of("LOUDNESS");
            let mut offset = 0.0;
            for choice in ExportLoudness::CHOICES {
                let width = if choice == ExportLoudness::Off {
                    58.0
                } else {
                    80.0
                };
                out.push((
                    Hit::Loudness(choice),
                    self.button_at(loud_row, offset, width),
                    choice.label(),
                    v.loudness == choice,
                    true,
                ));
                offset += width + BTN_GAP;
            }
            let extra_row = self.row_of("EXTRA FILE");
            let extra_on = !v.file.is_lossy() && self.lossy_enabled();
            out.push((
                Hit::Extra(false),
                self.button(extra_row, 0, 76.0),
                "ONE FILE".to_owned(),
                !v.also_mp3 || !extra_on,
                true,
            ));
            out.push((
                Hit::Extra(true),
                self.button_at(extra_row, 76.0 + BTN_GAP, 130.0),
                format!("MP3 {ALSO_MP3_KBPS} kbps"),
                v.also_mp3 && extra_on,
                extra_on,
            ));
        }
        out.push((
            Hit::Start,
            self.footer_button(1),
            "EXPORT".to_owned(),
            false,
            true,
        ));
        out.push((
            Hit::Cancel,
            self.footer_button(0),
            "CANCEL".to_owned(),
            false,
            true,
        ));
        out
    }

    /// Whether the name typed can be saved: it is not blank.
    fn name_ok(&self) -> bool {
        self.naming
            .as_ref()
            .is_some_and(|f| !f.text().trim().is_empty())
    }

    /// The button labelled `label` (`"EXPORT"`, `"24-BIT"`, `"44.1 kHz"`), for tests and for the
    /// headless session to press.
    pub fn button_rect(&self, label: &str) -> Option<Rect> {
        self.buttons()
            .into_iter()
            .find(|b| b.2 == label)
            .map(|b| b.1)
    }

    fn hit(&self, pos: Point) -> Option<Hit> {
        self.buttons()
            .into_iter()
            .find(|(_, r, _, _, enabled)| *enabled && r.contains(pos))
            .map(|(h, ..)| h)
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                if let Some(list) = &mut self.list {
                    list.hover = list.row_at(*pos);
                }
                if self.selecting
                    && let Some(field) = &mut self.naming
                {
                    field.drag(*pos);
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.selecting = false,
            UiEvent::Wheel { delta, .. } => {
                if let Some(list) = &mut self.list
                    && list.wheel(delta.y)
                {
                    self.repaint = true;
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if self.list.is_some() {
                    self.press_list(*pos);
                } else if self.naming.is_some() && self.name_rect().contains(*pos) {
                    if let Some(field) = &mut self.naming {
                        field.press(*pos, *clicks, modifiers.shift);
                        self.selecting = true;
                    }
                } else if let Some(hit) = self.hit(*pos) {
                    out.extend(self.press(hit).map(UiAction::Export));
                }
            }
            UiEvent::Ime(ime) => {
                if let Some(field) = &mut self.naming
                    && field.handle_ime(ime)
                {
                    self.repaint = true;
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } if self.naming.is_some() => {
                self.repaint = true;
                let used = self
                    .naming
                    .as_mut()
                    .map(|f| f.handle_key(key, *modifiers))
                    .is_some_and(|u| u.handled());
                if !used {
                    match key {
                        Key::Escape => self.naming = None,
                        Key::Enter if self.name_ok() => {
                            out.push(UiAction::Export(ExportAction::SavePreset));
                        }
                        _ => {}
                    }
                }
            }
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } if self.list.is_some() => {
                self.list = None;
                self.repaint = true;
            }
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => out.push(UiAction::Export(ExportAction::Cancel)),
            UiEvent::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } if !self.in_progress() && self.list.is_none() => {
                out.extend(self.start().map(UiAction::Export));
            }
            _ => {}
        }
        out
    }

    /// A press while the preset list is open: a row chooses that preset (one that cannot be used
    /// leaves the list open), anything else closes the list.
    fn press_list(&mut self, pos: Point) {
        let Some(list) = self.list.clone() else {
            return;
        };
        match list.row_at(pos) {
            Some(i) => {
                if let Some(name) = self.presets.get(i).map(|p| p.name.clone()) {
                    self.choose_preset(&name);
                }
            }
            None => self.list = None,
        }
    }

    fn press(&mut self, hit: Hit) -> Option<ExportAction> {
        match hit {
            Hit::PresetList => {
                self.list = Some(self.list_at());
                return None;
            }
            Hit::PresetSave => {
                let text = if self.preset_is_user() {
                    self.preset.clone().unwrap_or_default()
                } else {
                    String::new()
                };
                let mut field = TextField::new(&text, 40, false);
                field.select_all();
                self.naming = Some(field);
                return None;
            }
            Hit::PresetDelete => return Some(ExportAction::DeletePreset),
            Hit::NameOk => return self.name_ok().then_some(ExportAction::SavePreset),
            Hit::NameCancel => {
                self.naming = None;
                return None;
            }
            Hit::Mono(m) => self.values.mono = m,
            Hit::Loudness(l) => self.values.loudness = l,
            Hit::Extra(e) => self.values.also_mp3 = e,
            Hit::Depth(d) => self.values.depth = d,
            Hit::Dither(d) => self.values.dither = d,
            Hit::Rate(r) => self.values.rate = r,
            Hit::File(f) => {
                self.values.file = f;
                if f == ExportFile::Flac && self.values.depth == ExportDepth::Float32 {
                    self.values.depth = ExportDepth::Pcm24;
                }
            }
            Hit::Quality(q) => match (self.values.file, q) {
                (ExportFile::Mp3, q) => self.values.mp3 = q,
                (ExportFile::Aac, ExportQuality::Cbr(k)) => self.values.aac = k,
                (ExportFile::Opus, ExportQuality::Cbr(k)) => self.values.opus = k,
                _ => {}
            },
            Hit::Tail => self.values.tail = !self.values.tail,
            Hit::Dry(d) => self.values.dry = d,
            Hit::Range(r) => {
                self.values.locators = r == RangeChoice::Locators;
                self.values.selection = r == RangeChoice::Selection;
            }
            Hit::Start => return self.start(),
            Hit::Cancel => return Some(ExportAction::Cancel),
        }
        // A changed choice is a new question: the old line no longer applies.
        self.warning = None;
        None
    }

    /// EXPORT: the values, unless a compressed format waits for FFmpeg to be found.
    fn start(&self) -> Option<ExportAction> {
        let waiting = self.values.file.is_lossy() && self.encoder != EncoderState::Ready;
        let mut values = self.values;
        // What a row that is not shown or not usable says does not count.
        if self.scope == ExportScope::Stems {
            values.mono = false;
            values.loudness = ExportLoudness::Off;
            values.also_mp3 = false;
        }
        if values.file.is_lossy() {
            values.also_mp3 = false;
        }
        (!waiting).then_some(ExportAction::Start(values))
    }

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        // A greyed button still says why it is greyed.
        let (hit, rect) = self
            .buttons()
            .into_iter()
            .find(|b| b.1.contains(pos))
            .map(|b| (b.0, b.1))?;
        if let (Hit::File(file), EncoderState::Missing(why)) = (hit, &self.encoder)
            && file.is_lossy()
            && self.scope != ExportScope::Stems
        {
            return Some(Help::new(rect, why.clone()));
        }
        let text = match hit {
            Hit::Depth(_) | Hit::Dither(_) if self.values.file.is_lossy() => {
                "Not used for a compressed file: the encoder decides the precision"
            }
            Hit::Depth(ExportDepth::Pcm16) => {
                "16-bit integer file, as a compact disc: dither it to keep quiet detail"
            }
            Hit::Depth(ExportDepth::Pcm24) => "24-bit integer file: no dither is needed",
            Hit::Depth(ExportDepth::Float32) if self.values.file == ExportFile::Flac => {
                "FLAC holds 16-bit and 24-bit audio: pick WAV or AIFF for 32-bit float"
            }
            Hit::Depth(ExportDepth::Float32) => "32-bit float file: the mix is written untouched",
            Hit::Dither(ExportDither::Off) => "Round to 16 bits without dither",
            Hit::Dither(ExportDither::Tpdf) => {
                "Add triangular dither before rounding to 16 bits (the usual choice)"
            }
            Hit::Dither(ExportDither::Shaped) => {
                "Triangular dither with the noise moved towards the high frequencies"
            }
            Hit::Rate(None) => "Write the file at the sample rate of the project",
            Hit::Rate(Some(_)) => "Convert the render to this sample rate",
            Hit::File(ExportFile::Wav) => "Write WAV files",
            Hit::File(ExportFile::Aiff) => "Write AIFF files (AIFF-C for 32-bit float)",
            Hit::File(ExportFile::Flac) => {
                "Write FLAC files: lossless and smaller, at 16 or 24 bits"
            }
            Hit::File(_) if self.scope == ExportScope::Stems => {
                "Stems are written as WAV, AIFF or FLAC files"
            }
            Hit::File(ExportFile::Mp3) => {
                "Write MP3 files with FFmpeg: small, plays everywhere, lossy"
            }
            Hit::File(ExportFile::Aac) => {
                "Write AAC files (.m4a) with FFmpeg: small and clean at a modest bitrate, lossy"
            }
            Hit::File(_) => "Write Opus files with FFmpeg: the best sound for the size, lossy",
            Hit::Quality(ExportQuality::Vbr(0)) => {
                "Variable bitrate, best quality (about 245 kbps): the size follows the music"
            }
            Hit::Quality(ExportQuality::Vbr(2)) => {
                "Variable bitrate, high quality (about 190 kbps): the size follows the music"
            }
            Hit::Quality(ExportQuality::Vbr(_)) => {
                "Variable bitrate, good quality (about 165 kbps): the size follows the music"
            }
            Hit::Quality(ExportQuality::Cbr(_)) => {
                "Constant bitrate in kilobits per second: higher is clearer and bigger"
            }
            Hit::Tail => {
                "Keep what rings out after the End marker (reverb, delay) while it is audible"
            }
            Hit::Dry(false) => {
                "Each channel as it sounds in the mix: fader, pan and inserts applied"
            }
            Hit::Dry(true) => {
                "Each channel's source before its strip: no fader, pan, insert or send; buses, \
                 returns and the master are left out"
            }
            Hit::Range(RangeChoice::Song) => {
                "Every strip from the start to the End marker, plus the tail"
            }
            Hit::Range(RangeChoice::Locators) => {
                "Only the range between the left and the right locator, with no tail"
            }
            Hit::Range(RangeChoice::Selection) => {
                "Only the time range selected on the lanes, with no tail"
            }
            Hit::PresetList => {
                "Fill the options for a purpose: streaming, podcast, video, stems, club or your own"
            }
            Hit::PresetSave => "Keep the options as a preset of your own under a name you type",
            Hit::PresetDelete => "Delete the preset shown (only presets you saved)",
            Hit::NameOk => "Save the options as a preset under this name",
            Hit::NameCancel => "Go back without saving a preset",
            Hit::Mono(false) => "Keep the channels of the project's output",
            Hit::Mono(true) => "Mix every channel into one before writing",
            Hit::Loudness(ExportLoudness::Off) => "Leave the level as the song has it",
            Hit::Loudness(ExportLoudness::Target(_)) => {
                "Set the loudness to this target; a limiter keeps the peaks under -1 dBTP"
            }
            Hit::Loudness(ExportLoudness::Guide(_)) => {
                "A guide only: report the distance and warn on clipping, change nothing"
            }
            Hit::Extra(false) => "Write the one file",
            Hit::Extra(true) => {
                "Also write an MP3 from the same render, next to the file (needs FFmpeg)"
            }
            Hit::Start if self.scope == ExportScope::Stems => "Render and write the files",
            Hit::Start => "Render and write the file",
            Hit::Cancel if self.in_progress() => "Stop the render and write nothing",
            Hit::Cancel => "Close without exporting",
        };
        Some(Help::new(rect, text))
    }

    // ------------------------------------------------------------------ painting

    fn title(&self) -> &'static str {
        match (self.scope, self.in_progress()) {
            (ExportScope::Song, false) => "EXPORT SONG AS AUDIO FILE",
            (ExportScope::Loop, false) => "EXPORT LOOP AS AUDIO FILE",
            (ExportScope::Selection, false) => "EXPORT SELECTION AS AUDIO FILE",
            (ExportScope::Stems, false) => "EXPORT STEMS",
            (ExportScope::Song, true) => "EXPORTING SONG",
            (ExportScope::Loop, true) => "EXPORTING LOOP",
            (ExportScope::Selection, true) => "EXPORTING SELECTION",
            (ExportScope::Stems, true) => "EXPORTING STEMS",
        }
    }

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            self.title(),
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let width = p.width() - 2.0 * PAD;
        sp::label(
            c,
            &fit_label(&self.file, width, 11.0),
            p.x0 + PAD,
            p.y0 + HEADER_H + 17.0,
            11.0,
            t.text,
            Align::Start,
        );
        match &self.mode {
            Mode::Options => self.paint_options(c, p),
            Mode::Progress { fraction, text } => {
                sp::label(
                    c,
                    &fit_label(text, width, 10.0),
                    p.x0 + PAD,
                    p.y0 + HEADER_H + 30.0,
                    10.0,
                    t.text_muted,
                    Align::Start,
                );
                let bar = self.bar_rect();
                sp::rect(c, bar, st.lane);
                let filled = Rect::new(
                    bar.x0,
                    bar.y0,
                    bar.x0 + bar.width() * f64::from(*fraction),
                    bar.y1,
                );
                sp::rect(c, filled, t.highlight);
                let percent = (fraction * 100.0).round() as u32;
                sp::label(
                    c,
                    &format!("{percent}%"),
                    bar.x1,
                    bar.y1 + 14.0,
                    10.0,
                    t.text_muted,
                    Align::End,
                );
            }
        }
        for (_, rect, text, lit, enabled) in self.buttons() {
            let color = (lit && enabled).then_some(t.highlight);
            if enabled {
                sp::button(c, st, rect, &text, color, hovered(rect));
            } else {
                sp::button(c, st, rect, "", None, false);
                sp::label(
                    c,
                    &text,
                    rect.center().x,
                    rect.center().y + 3.5,
                    9.5,
                    t.text_muted,
                    Align::Center,
                );
            }
        }
        self.paint_presets(c);
        c.pop_clip();
        c.pop_transform();
    }

    /// The name field, the drop-down's arrow and the open list of the Preset row.
    fn paint_presets(&self, c: &mut dyn Canvas) {
        if self.in_progress() {
            return;
        }
        let st = &self.st;
        let t = &st.base;
        if let Some(field) = &self.naming {
            let r = self.name_rect();
            sp::lcd(c, st, r, Some(t.focus));
            field.paint(c, r, 10.5, 14.0, FieldColors::of(t), true);
        } else {
            let r = self.preset_rect();
            let (x, y) = (r.x1 - 10.0, r.center().y);
            sp::line(c, x - 3.5, y - 1.5, x, y + 2.0, t.text_muted);
            sp::line(c, x, y + 2.0, x + 3.5, y - 1.5, t.text_muted);
        }
        let Some(list) = &self.list else {
            return;
        };
        list.paint_frame(c, st);
        for i in list.visible() {
            let (Some(rr), Some(preset)) = (list.row_rect(i), self.presets.get(i)) else {
                continue;
            };
            let usable = self.preset_usable(preset);
            if list.hover == Some(i) && usable {
                c.fill_with(RoundedRect::from_rect(rr, 3.0), t.browser_row);
            }
            let text = if usable {
                preset.name.clone()
            } else {
                format!("{} (Needs FFmpeg)", preset.name)
            };
            let color = if !usable {
                t.text_muted
            } else if self.preset.as_deref() == Some(preset.name.as_str()) {
                t.highlight
            } else {
                t.text
            };
            sp::label(
                c,
                &fit_label(&text, list.width - 2.0 * pick_list::PAD - 14.0, 10.0),
                rr.x0 + 6.0,
                rr.y0 + 14.0,
                10.0,
                color,
                Align::Start,
            );
        }
        list.paint_scrollbar(c, st);
    }

    fn paint_options(&self, c: &mut dyn Canvas, p: Rect) {
        let t = &self.st.base;
        for (i, name) in self.row_names().iter().enumerate() {
            sp::label(
                c,
                name,
                p.x0 + PAD,
                self.row_top(i) + ROW_H / 2.0 + 3.5,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        let hint = match self.scope {
            ExportScope::Song => {
                "From the start to the End marker; the tail is kept while audible."
            }
            ExportScope::Loop => "Exactly from the left to the right locator, with no tail.",
            ExportScope::Selection => "Exactly the time range selected on the lanes, with no tail.",
            ExportScope::Stems if self.values.selection => {
                "One file per strip, exactly the selected range, lined up with the master."
            }
            ExportScope::Stems if self.values.locators => {
                "One file per strip, exactly between the locators, lined up with the master."
            }
            ExportScope::Stems => "One file per strip, in order, lined up with the master.",
        };
        let note = if self.values.file.is_lossy() {
            "Lossy encoding can add peaks: leave 1 dB of true-peak headroom.".to_owned()
        } else if self.values.depth == ExportDepth::Pcm16 {
            hint.to_owned()
        } else if self.values.depth == ExportDepth::Pcm24 {
            format!("{hint} 24-bit needs no dither.")
        } else {
            format!("{hint} Float is written as it is.")
        };
        sp::label(
            c,
            &fit_label(&note, p.width() - 2.0 * PAD, 10.0),
            p.x0 + PAD,
            self.row_top(self.rows()) + 14.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
        let encoder_line = match (&self.warning, &self.encoder) {
            (Some(warning), _) => Some(warning.as_str()),
            (None, EncoderState::Ready) => None,
            (None, EncoderState::Checking) => {
                Some("Looking for FFmpeg (MP3, AAC and Opus need it)...")
            }
            (None, EncoderState::Missing(why)) => Some(why.as_str()),
        };
        if let Some(line) = encoder_line {
            sp::label(
                c,
                &fit_label(line, p.width() - 2.0 * PAD, 10.0),
                p.x0 + PAD,
                self.row_top(self.rows()) + 26.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        sp::label(
            c,
            &format!("Project rate {}", rate_label(self.project_rate)),
            p.x0 + PAD,
            p.y1 - FOOTER_H / 2.0 + 3.5,
            10.0,
            t.text_muted,
            Align::Start,
        );
    }
}

/// A rate as a button label: `44.1 kHz`, `48 kHz`.
pub fn rate_label(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1000.0)
    }
}
