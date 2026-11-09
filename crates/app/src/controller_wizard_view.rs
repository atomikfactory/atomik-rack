//! The screen of the controller wizard (Settings > Controllers > Set Up Controller...): a modal
//! that first lists the MIDI input ports, then walks through the controls ("Move knob 1", ...,
//! "Press Play") with Skip, Skip Group, Back and Finish, asks whether the controller shows values
//! back, and ends with Save.
//!
//! Everything that decides something is in [`crate::controller_wizard`]; this view owns the
//! port's link (opened by the shell when a port is picked), polls it every frame and draws.

use crate::controller_profile::ControllerProfile;
use crate::controller_wizard::{Progress, Stage, Wizard};
use crate::surface::SurfaceLink;
use atomik_core::{ChannelMessage, MessageKind};
use atomik_midi_io::{RunningStatus, parse_message};
use atomik_ui::kurbo::{Point, Rect, RoundedRect, Size};
use atomik_ui::paint::fit_label_in;
use atomik_ui::peniko::Color;
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 380.0;
const PAD: f64 = 24.0;
const ROW: f64 = 28.0;
const BUTTON: (f64, f64) = (104.0, 28.0);
const GAP: f64 = 10.0;
/// The most ports the list shows.
const VISIBLE_PORTS: usize = 8;

/// A button or row of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The input port at this place of the list.
    Port(usize),
    /// Leave this control out.
    Skip,
    /// Leave this control and the rest of its kind out.
    SkipGroup,
    /// Take the last answer back.
    Back,
    /// Finish with what was learned.
    Finish,
    /// Answer yes to the question.
    Yes,
    /// Answer no to the question.
    No,
    /// Save the profile.
    Save,
    /// Close without saving.
    Cancel,
}

/// What the screen asks of the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardOutcome {
    /// Still open.
    Open,
    /// Close without saving.
    Close,
    /// The user picked this input port: open it and call [`ControllerWizardView::begin`].
    PickPort(String),
    /// Save this profile and close.
    Save(ControllerProfile),
}

struct Learning {
    wizard: Wizard,
    link: Box<dyn SurfaceLink>,
    running: RunningStatus,
    /// The last message heard, as text.
    heard: Option<String>,
}

enum Phase {
    Ports,
    Learning(Box<Learning>),
}

/// The wizard's modal.
pub struct ControllerWizardView {
    phase: Phase,
    inputs: Vec<String>,
    outputs: Vec<String>,
    hover: Option<Target>,
    focus: usize,
    /// A problem to show (a port that would not open).
    problem: Option<String>,
}

impl std::fmt::Debug for ControllerWizardView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControllerWizardView")
            .field("inputs", &self.inputs)
            .finish_non_exhaustive()
    }
}

/// A port name without its direction words and case: `Desk In` and `desk out` both give `desk`.
fn stem(name: &str) -> String {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !matches!(*t, "in" | "out" | "input" | "output"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The output port that goes with the input port `input`: the one of the same name, else the one
/// that differs only by its direction word (`Desk In`, `Desk Out`), else the one whose name
/// contains it or is contained in it; the first match that is the only one wins, so two ports
/// that both fit give none.
pub fn matching_output(input: &str, outputs: &[String]) -> Option<String> {
    if let Some(same) = outputs.iter().find(|o| o.as_str() == input) {
        return Some(same.clone());
    }
    let wanted = stem(input);
    if wanted.is_empty() {
        return None;
    }
    let only = |keep: &dyn Fn(&str) -> bool| {
        let mut it = outputs.iter().filter(|o| keep(&stem(o)));
        match (it.next(), it.next()) {
            (Some(one), None) => Some(Some(one.clone())),
            (Some(_), Some(_)) => Some(None),
            _ => None,
        }
    };
    only(&|o| o == wanted)
        .or_else(|| only(&|o| !o.is_empty() && (o.contains(&wanted) || wanted.contains(o))))
        .flatten()
}

/// A message as short text: `CC 21 = 64 (channel 1)`.
pub fn describe_message(m: &ChannelMessage) -> String {
    let ch = u16::from(m.channel) + 1;
    match m.kind {
        MessageKind::Cc { number, value } => format!("CC {number} = {value} (channel {ch})"),
        MessageKind::NoteOn { key, velocity } => {
            format!(
                "note {} on, velocity {velocity} (channel {ch})",
                atomik_model::note_name(key)
            )
        }
        MessageKind::NoteOff { key } => {
            format!("note {} off (channel {ch})", atomik_model::note_name(key))
        }
        MessageKind::PitchBend { value } => format!("pitch bend {value} (channel {ch})"),
    }
}

impl ControllerWizardView {
    /// The screen at its first step: the list of input ports (`outputs` are used to find the
    /// feedback port of the one picked).
    pub fn new(inputs: Vec<String>, outputs: Vec<String>) -> Self {
        Self {
            phase: Phase::Ports,
            inputs,
            outputs,
            hover: None,
            focus: 0,
            problem: None,
        }
    }

    /// Starts learning on `link` for the controller on `input` (the shell opened the port).
    pub fn begin(&mut self, link: Box<dyn SurfaceLink>, input: &str) {
        let output = matching_output(input, &self.outputs);
        self.problem = None;
        self.phase = Phase::Learning(Box::new(Learning {
            wizard: Wizard::new(input, output.as_deref()),
            link,
            running: RunningStatus::default(),
            heard: None,
        }));
    }

    /// The port could not be opened: say so and stay on the list.
    pub fn fail(&mut self, problem: String) {
        self.problem = Some(problem);
        self.phase = Phase::Ports;
    }

    /// True while a controller is being learned.
    pub fn learning(&self) -> bool {
        matches!(self.phase, Phase::Learning(_))
    }

    /// The wizard, while a controller is being learned.
    pub fn wizard(&self) -> Option<&Wizard> {
        match &self.phase {
            Phase::Learning(l) => Some(&l.wizard),
            Phase::Ports => None,
        }
    }

    /// Takes what the port sent since the last call; true when the screen changed.
    pub fn poll(&mut self) -> bool {
        let Phase::Learning(l) = &mut self.phase else {
            return false;
        };
        let mut incoming = Vec::new();
        l.link.poll(&mut incoming);
        let mut changed = false;
        for bytes in incoming {
            let Some(msg) = parse_message(&bytes, &mut l.running) else {
                continue;
            };
            l.heard = Some(describe_message(&msg));
            changed = true;
            let _ = l.wizard.feed(&msg);
        }
        changed
    }

    /// Feeds a message as if the port had sent it (tests and the headless session).
    pub fn feed(&mut self, msg: &ChannelMessage) -> Option<Progress> {
        let Phase::Learning(l) = &mut self.phase else {
            return None;
        };
        l.heard = Some(describe_message(msg));
        Some(l.wizard.feed(msg))
    }

    fn rect(&self, view: Size) -> Rect {
        let size = Size::new(WIDTH.min(view.width - 20.0), HEIGHT.min(view.height - 20.0));
        Rect::from_center_size(Point::new(view.width / 2.0, view.height / 2.0), size)
    }

    fn port_rows(&self, view: Size) -> Vec<(Target, Rect)> {
        let r = self.rect(view);
        (0..self.inputs.len().min(VISIBLE_PORTS))
            .map(|i| {
                let y = r.y0 + 110.0 + i as f64 * ROW;
                (
                    Target::Port(i),
                    Rect::new(r.x0 + PAD, y, r.x1 - PAD, y + ROW - 2.0),
                )
            })
            .collect()
    }

    /// The buttons now, with their labels, laid out along the foot of the box.
    fn buttons(&self, view: Size) -> Vec<(Target, Rect, &'static str)> {
        let r = self.rect(view);
        let y = r.y1 - 20.0 - BUTTON.1;
        let make = |x: f64, t: Target, label: &'static str| {
            (t, Rect::new(x, y, x + BUTTON.0, y + BUTTON.1), label)
        };
        let mut v = vec![make(r.x0 + PAD, Target::Cancel, "Cancel")];
        let right: Vec<(Target, &'static str)> = match &self.phase {
            Phase::Ports => Vec::new(),
            Phase::Learning(l) => match l.wizard.stage() {
                Stage::Control(_) => vec![
                    (Target::Back, "Back"),
                    (Target::Skip, "Skip"),
                    (Target::SkipGroup, "Skip Group"),
                    (Target::Finish, "Finish"),
                ],
                Stage::Ask(_) => vec![
                    (Target::Back, "Back"),
                    (Target::No, "No"),
                    (Target::Yes, "Yes"),
                ],
                Stage::Done => vec![(Target::Back, "Back"), (Target::Save, "Save")],
            },
        };
        let mut x = r.x1 - PAD - BUTTON.0;
        for (target, label) in right.into_iter().rev() {
            v.push(make(x, target, label));
            x -= BUTTON.0 + GAP;
        }
        v
    }

    /// The middle of the button or port row `target` for a window of size `view`, when it is on
    /// the screen now (tests and the headless session click it).
    pub fn center_of(&self, view: Size, target: Target) -> Option<Point> {
        let ports = if self.learning() {
            Vec::new()
        } else {
            self.port_rows(view)
        };
        ports
            .into_iter()
            .chain(self.buttons(view).into_iter().map(|(t, r, _)| (t, r)))
            .find(|(t, _)| *t == target)
            .map(|(_, r)| r.center())
    }

    fn target_at(&self, view: Size, p: Point) -> Option<Target> {
        let ports = if self.learning() {
            Vec::new()
        } else {
            self.port_rows(view)
        };
        ports
            .into_iter()
            .chain(self.buttons(view).into_iter().map(|(t, r, _)| (t, r)))
            .find(|(_, r)| r.contains(p))
            .map(|(t, _)| t)
    }

    /// What picking `target` does.
    fn pick(&mut self, target: Target) -> WizardOutcome {
        match target {
            Target::Cancel => return WizardOutcome::Close,
            Target::Port(i) => {
                if let Some(name) = self.inputs.get(i) {
                    return WizardOutcome::PickPort(name.clone());
                }
            }
            _ => {}
        }
        let Phase::Learning(l) = &mut self.phase else {
            return WizardOutcome::Open;
        };
        let mut leave = false;
        match target {
            Target::Skip => l.wizard.skip(),
            Target::SkipGroup => l.wizard.skip_group(),
            Target::Back => {
                if l.wizard.count() == 0 && l.wizard.position().0 == 0 {
                    // Nothing to take back: go back to the list of ports.
                    leave = true;
                } else {
                    l.wizard.back();
                }
            }
            Target::Finish => l.wizard.finish_now(),
            Target::Yes => l.wizard.answer(true),
            Target::No => l.wizard.answer(false),
            Target::Save => {
                let name = profile_name(l.wizard.input());
                if let Some(profile) = l.wizard.profile(&name) {
                    return WizardOutcome::Save(profile);
                }
                self.problem = Some("No control was set up: nothing to save".to_owned());
            }
            Target::Port(_) | Target::Cancel => {}
        }
        if leave {
            self.phase = Phase::Ports;
        }
        WizardOutcome::Open
    }

    /// Handles one event; the modal swallows them all.
    pub fn handle(&mut self, event: &UiEvent, view: Size) -> WizardOutcome {
        match event {
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                if modifiers.ctrl || modifiers.alt || modifiers.meta {
                    return WizardOutcome::Open;
                }
                match key {
                    Key::Escape => return WizardOutcome::Close,
                    Key::Up if !self.learning() => self.focus = self.focus.saturating_sub(1),
                    Key::Down if !self.learning() => {
                        self.focus = (self.focus + 1).min(self.inputs.len().saturating_sub(1));
                    }
                    Key::Enter => {
                        let target = match self.wizard().map(Wizard::stage) {
                            None => Target::Port(self.focus),
                            Some(Stage::Done) => Target::Save,
                            Some(Stage::Ask(_)) => Target::Yes,
                            Some(Stage::Control(_)) => return WizardOutcome::Open,
                        };
                        return self.pick(target);
                    }
                    Key::Backspace if self.learning() => return self.pick(Target::Back),
                    Key::Char('s' | 'S') if self.learning() => return self.pick(Target::Skip),
                    Key::Char('n' | 'N') if self.learning() => return self.pick(Target::No),
                    Key::Char('y' | 'Y') if self.learning() => return self.pick(Target::Yes),
                    _ => {}
                }
            }
            UiEvent::PointerMove { pos, .. } => self.hover = self.target_at(view, *pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if let Some(target) = self.target_at(view, *pos) {
                    return self.pick(target);
                }
                if !self.rect(view).contains(*pos) {
                    return WizardOutcome::Close;
                }
            }
            _ => {}
        }
        WizardOutcome::Open
    }

    /// The text of the box, in drawing order (for tests and the headless session).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec!["Set Up Controller".to_owned()];
        match &self.phase {
            Phase::Ports => {
                lines.push("Choose the MIDI input port of your controller.".to_owned());
                if self.inputs.is_empty() {
                    lines.push(
                        "No MIDI input ports found. Plug the controller in and open this again."
                            .to_owned(),
                    );
                }
                lines.extend(self.inputs.iter().take(VISIBLE_PORTS).cloned());
            }
            Phase::Learning(l) => {
                lines.push(format!("Port: {}", l.wizard.input()));
                lines.push(l.wizard.prompt());
                let (at, all) = l.wizard.position();
                if matches!(l.wizard.stage(), Stage::Control(_)) {
                    lines.push(format!("Control {} of {all}", at + 1));
                }
                if let Some(h) = l.wizard.hint() {
                    lines.push(h.to_owned());
                }
                if let Some(h) = &l.heard {
                    lines.push(format!("Heard: {h}"));
                }
                lines.push(format!("{} controls set up", l.wizard.count()));
                if l.wizard.stage() == Stage::Done {
                    lines.push(format!("Name: {}", profile_name(l.wizard.input())));
                }
            }
        }
        if let Some(p) = &self.problem {
            lines.push(p.clone());
        }
        lines
    }

    /// Paints the dimmed backdrop and the box.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, view: Size) {
        canvas.fill_with(
            Rect::from_origin_size(Point::ZERO, view),
            Color::from_rgba8(0, 0, 0, 140),
        );
        let r = self.rect(view);
        let panel = RoundedRect::from_rect(r, 10.0);
        canvas.fill_with(panel, theme.browser_bg);
        canvas.stroke_with(panel, 1.0, theme.section_line);
        canvas.text(
            &TextRun::new(
                "Set Up Controller",
                Point::new(r.x0 + PAD, r.y0 + 30.0),
                18.0,
                theme.text,
            )
            .weight(700),
        );
        let text = |canvas: &mut dyn Canvas, s: &str, y: f64, size: f32, color: Color| {
            let fitted = fit_label_in(canvas, s, r.width() - 2.0 * PAD, size);
            canvas.text(&TextRun::new(
                fitted,
                Point::new(r.x0 + PAD, y),
                size,
                color,
            ));
        };
        match &self.phase {
            Phase::Ports => {
                text(
                    canvas,
                    "Choose the MIDI input port of your controller.",
                    r.y0 + 62.0,
                    13.0,
                    theme.text_muted,
                );
                if self.inputs.is_empty() {
                    text(
                        canvas,
                        "No MIDI input ports found. Plug the controller in and open this again.",
                        r.y0 + 130.0,
                        12.5,
                        theme.text_muted,
                    );
                }
                for (target, rect) in self.port_rows(view) {
                    let Target::Port(i) = target else { continue };
                    let shape = RoundedRect::from_rect(rect, 6.0);
                    if self.hover == Some(target) {
                        canvas.fill_with(shape, theme.selection);
                    }
                    if self.focus == i {
                        canvas.stroke_with(shape, 1.5, theme.focus);
                    }
                    let label = fit_label_in(canvas, &self.inputs[i], rect.width() - 20.0, 13.0);
                    canvas.text(&TextRun::new(
                        label,
                        Point::new(rect.x0 + 10.0, rect.y0 + 18.0),
                        13.0,
                        theme.text,
                    ));
                }
            }
            Phase::Learning(l) => {
                text(
                    canvas,
                    &format!("Port: {}", l.wizard.input()),
                    r.y0 + 58.0,
                    12.0,
                    theme.text_muted,
                );
                canvas.text(
                    &TextRun::new(
                        l.wizard.prompt(),
                        Point::new(r.x0 + PAD, r.y0 + 120.0),
                        22.0,
                        theme.text,
                    )
                    .weight(600),
                );
                let (at, all) = l.wizard.position();
                if matches!(l.wizard.stage(), Stage::Control(_)) {
                    text(
                        canvas,
                        &format!(
                            "Control {} of {all}. Skip a control the controller lacks.",
                            at + 1
                        ),
                        r.y0 + 150.0,
                        12.5,
                        theme.text_muted,
                    );
                }
                if let Some(h) = l.wizard.hint() {
                    text(canvas, h, r.y0 + 190.0, 13.0, theme.text);
                }
                if let Some(h) = &l.heard {
                    text(
                        canvas,
                        &format!("Heard: {h}"),
                        r.y0 + 215.0,
                        12.5,
                        theme.text_muted,
                    );
                }
                text(
                    canvas,
                    &format!("{} controls set up", l.wizard.count()),
                    r.y0 + 245.0,
                    12.5,
                    theme.text_muted,
                );
                if l.wizard.stage() == Stage::Done {
                    text(
                        canvas,
                        &format!("It will be saved as {}", profile_name(l.wizard.input())),
                        r.y0 + 275.0,
                        12.5,
                        theme.text,
                    );
                }
            }
        }
        if let Some(p) = &self.problem {
            text(canvas, p, r.y1 - 70.0, 12.5, theme.text);
        }
        for (target, rect, label) in self.buttons(view) {
            let shape = RoundedRect::from_rect(rect, 6.0);
            canvas.fill_with(
                shape,
                if self.hover == Some(target) {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    label,
                    Point::new(rect.center().x, rect.y0 + 18.5),
                    13.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        }
    }
}

/// The name a profile gets from its input port.
pub fn profile_name(input: &str) -> String {
    let name: String = input
        .trim()
        .chars()
        .take(crate::controller_profile::MAX_NAME_CHARS)
        .collect();
    if crate::controller_profile::slug(&name).is_empty() {
        "Controller".to_owned()
    } else {
        name
    }
}

#[cfg(test)]
#[path = "controller_wizard_view_tests.rs"]
mod tests;
