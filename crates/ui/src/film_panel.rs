//! The film export panel: the options of File > Export > Film with Mix, and the progress of the
//! render and the encoding that follow.
//!
//! ```text
//! EXPORT FILM WITH MIX
//! my song.mp4
//!   COVER      [ FILM'S SPAN ] [ WHOLE SONG ]
//!   PICTURE    [ AUTO ] [ COPY ] [ RE-ENCODE ]
//!   QUALITY    [ HIGH ] [ STANDARD ] [ SMALL ]
//!   SOUND      [ AAC ] [ PCM ]
//!   BITRATE    [ 128 kbps ] [ 192 kbps ] [ 256 kbps ] [ 320 kbps ]
//!   FILM AUDIO [ KEEP AS SECOND TRACK ]
//!   COPY and AUTO cut at keyframes: choose RE-ENCODE for exact cuts.
//!                                                          [ EXPORT ] [ CANCEL ]
//! ```
//!
//! A modal view the app places over the window like the audio export panel, with the same two
//! modes. In the **options** mode ([`FilmView::open`]) the buttons change the [`FilmValues`] the
//! view holds; EXPORT emits [`FilmAction::Start`] with them and CANCEL (or Esc) emits
//! [`FilmAction::Cancel`]. COVER says what the new film spans: the film as it is trimmed and
//! placed in the song (the mix is cut to it), or the whole song with black pictures before and
//! after the film. PICTURE says whether the picture is copied without re-encoding (AUTO does it
//! when the cut is at the start of the film, and COPY always, cutting at keyframes) or re-encoded
//! (cut exactly, with the QUALITY of the encoder). SOUND is AAC in an MP4 file or 24-bit PCM in a
//! MOV file. FILM AUDIO keeps the film's own sound as a second track. The panel says through
//! [`FilmView::set_encoder`] whether FFmpeg, which makes the film, can be used; without it a line
//! says how to set it up and EXPORT does nothing. In the **progress** mode
//! ([`FilmView::show_progress`]) a bar shows the fraction the app feeds with
//! [`FilmView::set_progress`], and CANCEL (or Esc) stops the job. The view never closes itself:
//! the app drops it when the export is over.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::export_panel::{AAC_BITRATES, EncoderState};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const FILE_H: f64 = 26.0;
const ROW_H: f64 = 32.0;
const HINT_H: f64 = 56.0;
const FOOTER_H: f64 = 44.0;
const BAR_H: f64 = 14.0;
const PROGRESS_H: f64 = 74.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 540.0;
const BTN_H: f64 = 22.0;
const BTN_GAP: f64 = 6.0;
const LABEL_W: f64 = 104.0;
const ROWS: usize = 6;

/// What the new film spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmCover {
    /// The film as it is trimmed and placed in the song: the mix is cut to it.
    Span,
    /// The whole song, with black pictures before and after the film.
    Song,
}

/// What happens to the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmPictureChoice {
    /// Copied when the cut is at the start of the film, else re-encoded.
    Auto,
    /// Always copied (cuts snap to keyframes).
    Copy,
    /// Always re-encoded, cut exactly.
    Encode,
}

/// How hard a re-encoded picture is squeezed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmQualityChoice {
    /// Nearly indistinguishable from the source, big.
    High,
    /// A good picture at a moderate size.
    Standard,
    /// Small, with visible softening.
    Small,
}

/// The sound of the new film.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmSoundChoice {
    /// AAC in an MP4 file.
    Aac,
    /// 24-bit PCM in a MOV file.
    Pcm,
}

impl FilmSoundChoice {
    /// The file extension of the container the sound goes in, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Aac => "mp4",
            Self::Pcm => "mov",
        }
    }
}

/// What the panel sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilmValues {
    /// What the film spans.
    pub cover: FilmCover,
    /// What happens to the picture.
    pub picture: FilmPictureChoice,
    /// The quality of a re-encoded picture.
    pub quality: FilmQualityChoice,
    /// The sound of the mix.
    pub sound: FilmSoundChoice,
    /// The AAC bitrate in kilobits per second.
    pub aac: u16,
    /// Keep the film's own sound as a second track.
    pub own_audio: bool,
}

impl Default for FilmValues {
    fn default() -> Self {
        Self {
            cover: FilmCover::Span,
            picture: FilmPictureChoice::Auto,
            quality: FilmQualityChoice::Standard,
            sound: FilmSoundChoice::Aac,
            aac: 192,
            own_audio: false,
        }
    }
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmAction {
    /// Start the export with these values.
    Start(FilmValues),
    /// Close the options, or stop the job that runs.
    Cancel,
}

/// A button of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Cover(FilmCover),
    Picture(FilmPictureChoice),
    Quality(FilmQualityChoice),
    Sound(FilmSoundChoice),
    Bitrate(u16),
    OwnAudio,
    Start,
    Cancel,
}

/// The panel mode.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Options,
    Progress { fraction: f32, text: String },
}

/// The film export panel. See the module documentation.
#[derive(Debug)]
pub struct FilmView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    file: String,
    has_audio: bool,
    values: FilmValues,
    encoder: EncoderState,
    warning: Option<String>,
    mode: Mode,
    hover: Option<Point>,
    repaint: bool,
}

impl FilmView {
    /// A panel in the options mode; call [`Self::open`] to set it up.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            file: String::new(),
            has_audio: true,
            values: FilmValues::default(),
            encoder: EncoderState::Ready,
            warning: None,
            mode: Mode::Options,
            hover: None,
            repaint: true,
        }
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
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

    /// Shows the options for the film that goes to the file called `file`, with the last
    /// `values`. `has_audio` says whether the song's film has sound of its own to keep.
    pub fn open(&mut self, file: &str, has_audio: bool, values: FilmValues) {
        self.file = file.to_owned();
        self.has_audio = has_audio;
        self.values = FilmValues {
            own_audio: values.own_audio && has_audio,
            ..values
        };
        self.warning = None;
        self.mode = Mode::Options;
        self.repaint = true;
    }

    /// Tells the panel whether FFmpeg can be used. Without it a line says how to set it up and
    /// EXPORT does nothing.
    pub fn set_encoder(&mut self, state: EncoderState) {
        if state != self.encoder {
            self.encoder = state;
            self.repaint = true;
        }
    }

    /// Shows a line under the options (the app says why it did not start, or what to confirm);
    /// it goes away when a button changes a choice.
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

    /// Switches to the progress bar (the job has started).
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
    pub fn values(&self) -> FilmValues {
        self.values
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let height = match self.mode {
            Mode::Options => HEADER_H + FILE_H + ROWS as f64 * ROW_H + HINT_H + FOOTER_H,
            Mode::Progress { .. } => HEADER_H + PROGRESS_H + FOOTER_H,
        };
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + height)
    }

    fn row_top(&self, row: usize) -> f64 {
        self.panel().y0 + HEADER_H + FILE_H + row as f64 * ROW_H
    }

    fn button_at(&self, row: usize, offset: f64, width: f64) -> Rect {
        let p = self.panel();
        let x = p.x0 + PAD + LABEL_W + offset;
        let y = self.row_top(row) + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + width, y + BTN_H)
    }

    fn footer_button(&self, from_right: usize) -> Rect {
        let p = self.panel();
        let x1 = p.x1 - PAD - from_right as f64 * (96.0 + BTN_GAP);
        let y = p.y1 - FOOTER_H + (FOOTER_H - BTN_H) / 2.0;
        Rect::new(x1 - 96.0, y, x1, y + BTN_H)
    }

    /// The progress bar.
    pub fn bar_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + 34.0;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + BAR_H)
    }

    /// Whether EXPORT can start now (FFmpeg is found).
    fn ready(&self) -> bool {
        self.encoder == EncoderState::Ready
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
        for (i, (cover, label, width)) in [
            (FilmCover::Span, "FILM'S SPAN", 110.0),
            (FilmCover::Song, "WHOLE SONG", 110.0),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::Cover(cover),
                self.button_at(0, i as f64 * 116.0, width),
                label.to_owned(),
                v.cover == cover,
                true,
            ));
        }
        for (i, (choice, label, width)) in [
            (FilmPictureChoice::Auto, "AUTO", 76.0),
            (FilmPictureChoice::Copy, "COPY", 76.0),
            (FilmPictureChoice::Encode, "RE-ENCODE", 96.0),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::Picture(choice),
                self.button_at(1, i as f64 * 82.0, width),
                label.to_owned(),
                v.picture == choice,
                true,
            ));
        }
        let quality_on = v.picture != FilmPictureChoice::Copy;
        for (i, (choice, label)) in [
            (FilmQualityChoice::High, "HIGH"),
            (FilmQualityChoice::Standard, "STANDARD"),
            (FilmQualityChoice::Small, "SMALL"),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::Quality(choice),
                self.button_at(2, i as f64 * 96.0, 90.0),
                label.to_owned(),
                quality_on && v.quality == choice,
                quality_on,
            ));
        }
        for (i, (choice, label)) in [
            (FilmSoundChoice::Aac, "AAC (MP4)"),
            (FilmSoundChoice::Pcm, "PCM (MOV)"),
        ]
        .into_iter()
        .enumerate()
        {
            out.push((
                Hit::Sound(choice),
                self.button_at(3, i as f64 * 106.0, 100.0),
                label.to_owned(),
                v.sound == choice,
                true,
            ));
        }
        let aac_on = v.sound == FilmSoundChoice::Aac;
        for (i, kbps) in AAC_BITRATES.into_iter().enumerate() {
            out.push((
                Hit::Bitrate(kbps),
                self.button_at(4, i as f64 * 82.0, 76.0),
                format!("{kbps} kbps"),
                aac_on && v.aac == kbps,
                aac_on,
            ));
        }
        out.push((
            Hit::OwnAudio,
            self.button_at(5, 0.0, 190.0),
            "KEEP AS SECOND TRACK".to_owned(),
            v.own_audio && self.has_audio,
            self.has_audio,
        ));
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

    /// The button labelled `label` (`"EXPORT"`, `"WHOLE SONG"`), for tests and for the headless
    /// session to press.
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
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.repaint = true;
                if let Some(hit) = self.hit(*pos) {
                    out.extend(self.press(hit).map(UiAction::Film));
                }
            }
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => out.push(UiAction::Film(FilmAction::Cancel)),
            UiEvent::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } if !self.in_progress() => {
                out.extend(self.start().map(UiAction::Film));
            }
            _ => {}
        }
        out
    }

    fn press(&mut self, hit: Hit) -> Option<FilmAction> {
        match hit {
            Hit::Cover(c) => self.values.cover = c,
            Hit::Picture(p) => self.values.picture = p,
            Hit::Quality(q) => self.values.quality = q,
            Hit::Sound(s) => self.values.sound = s,
            Hit::Bitrate(k) => self.values.aac = k,
            Hit::OwnAudio => self.values.own_audio = !self.values.own_audio,
            Hit::Start => return self.start(),
            Hit::Cancel => return Some(FilmAction::Cancel),
        }
        // A changed choice is a new question: the old line no longer applies.
        self.warning = None;
        None
    }

    /// EXPORT: the values, unless FFmpeg has not been found.
    fn start(&self) -> Option<FilmAction> {
        self.ready().then_some(FilmAction::Start(self.values))
    }

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let (hit, rect) = self
            .buttons()
            .into_iter()
            .find(|b| b.1.contains(pos))
            .map(|b| (b.0, b.1))?;
        if let (Hit::Start, EncoderState::Missing(why)) = (hit, &self.encoder) {
            return Some(Help::new(rect, why.clone()));
        }
        let text = match hit {
            Hit::Cover(FilmCover::Span) => {
                "Only the part of the song under the film, after its trim and start"
            }
            Hit::Cover(FilmCover::Song) => {
                "The whole song, with black pictures where the film is not playing"
            }
            Hit::Picture(FilmPictureChoice::Auto) => {
                "Copy the picture when the cut is at the film's start, else re-encode it"
            }
            Hit::Picture(FilmPictureChoice::Copy) => {
                "Copy the picture untouched: fast, but cuts snap to keyframes"
            }
            Hit::Picture(FilmPictureChoice::Encode) => {
                "Re-encode the picture to H.264: slower, and every cut is exact"
            }
            Hit::Quality(_) if self.values.picture == FilmPictureChoice::Copy => {
                "Not used while the picture is copied"
            }
            Hit::Quality(FilmQualityChoice::High) => {
                "H.264 at a high quality: a big file that looks like the source"
            }
            Hit::Quality(FilmQualityChoice::Standard) => {
                "H.264 at a standard quality: a good picture at a moderate size"
            }
            Hit::Quality(FilmQualityChoice::Small) => {
                "H.264 at a small size: smaller file, softer picture"
            }
            Hit::Sound(FilmSoundChoice::Aac) => "The mix as AAC in an MP4 file",
            Hit::Sound(FilmSoundChoice::Pcm) => "The mix as 24-bit PCM, untouched, in a MOV file",
            Hit::Bitrate(_) if self.values.sound == FilmSoundChoice::Pcm => {
                "Not used for PCM sound"
            }
            Hit::Bitrate(_) => "AAC bitrate in kilobits per second: higher is clearer and bigger",
            Hit::OwnAudio if !self.has_audio => "The song's film has no sound of its own",
            Hit::OwnAudio => "Keep the film's own sound as a second audio track after the mix",
            Hit::Start => "Render the mix and write the film",
            Hit::Cancel if self.in_progress() => "Stop the job and write nothing",
            Hit::Cancel => "Close without exporting",
        };
        Some(Help::new(rect, text))
    }

    // ------------------------------------------------------------------ painting

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
        let title = if self.in_progress() {
            "EXPORTING FILM"
        } else {
            "EXPORT FILM WITH MIX"
        };
        sp::label(
            c,
            title,
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
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_options(&self, c: &mut dyn Canvas, p: Rect) {
        let t = &self.st.base;
        for (i, name) in [
            "COVER",
            "PICTURE",
            "QUALITY",
            "SOUND",
            "BITRATE",
            "FILM AUDIO",
        ]
        .iter()
        .enumerate()
        {
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
        let width = p.width() - 2.0 * PAD;
        let top = self.row_top(ROWS);
        let cut = match self.values.picture {
            FilmPictureChoice::Encode => "Re-encoding cuts the picture exactly.",
            FilmPictureChoice::Auto => {
                "Copies cut at keyframes: AUTO copies only from the film's start, RE-ENCODE cuts exactly."
            }
            FilmPictureChoice::Copy => {
                "A copy cuts at keyframes: choose RE-ENCODE for an exact cut."
            }
        };
        let span = match self.values.cover {
            FilmCover::Span => "The mix starts at the first frame of the new film.",
            FilmCover::Song => "Black pictures cover the song before and after the film.",
        };
        let container = format!(
            "The film is written as {}.",
            self.values.sound.extension().to_uppercase()
        );
        let mut lines = vec![cut.to_owned(), format!("{span} {container}")];
        match &self.encoder {
            EncoderState::Ready => {}
            EncoderState::Checking => {
                lines.push("Looking for FFmpeg (the film is made with it)...".to_owned());
            }
            EncoderState::Missing(why) => lines.push(why.clone()),
        }
        if let Some(w) = &self.warning {
            lines.push(w.clone());
        }
        // The last two lines fit (a warning or the FFmpeg line replaces the explaining ones).
        let shown: Vec<String> = if lines.len() > 3 {
            lines.split_off(lines.len() - 3)
        } else {
            lines
        };
        for (i, line) in shown.iter().enumerate() {
            sp::label(
                c,
                &fit_label(line, width, 10.0),
                p.x0 + PAD,
                top + 14.0 + i as f64 * 14.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
    }
}
