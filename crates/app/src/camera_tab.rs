//! The Camera tab of the settings dialog: the webcam controller.
//!
//! The tab holds the **camera** to use (chosen from the list the system gave; the list is only
//! asked for when **Refresh** is pressed, so opening the dialog never touches a camera), the
//! **mode** (turn the clicked knob, send MIDI as the controller Camera, or both), the
//! **hand** that turns the knob (either, left or right), **Camera notes** (let the pinch and open
//! palm notes play the selected instrument too), **Twist for full range** (how far the
//! wrist turns to sweep a whole range, 90 to 360 degrees), **Fine control by distance** (a hand
//! near the camera turns the knob finer), **Pinch sensitivity** (scales how close the fingers must
//! come), **Show camera preview**, and **Delete downloaded models**. The values are workflow
//! preferences ([`crate::keymap_store::CameraPrefs`]) edited in a draft and merged back when the
//! dialog is applied; they are never saved in a project. The Interface and Workflow tabs have no
//! room left, so the camera has a tab of its own.

use crate::keyboard_tab::{PAD, TITLE, TabOutcome};
use crate::keymap_store::{
    CameraMode, CameraPrefs, HandChoice, MAX_FULL_TURN, MAX_SENSITIVITY, MIN_FULL_TURN,
    MIN_SENSITIVITY,
};
use crate::video_tab::wrap;
use crate::vision_link::{CameraEntry, ModelState};
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::{Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, UiEvent};

const ROW: f64 = 40.0;
const GAP: f64 = 2.0;
const STEP_W: f64 = 28.0;
const BUTTON_W: f64 = 64.0;
const MODE_W: f64 = 124.0;
/// How many degrees one press of the twist buttons moves, and with Shift.
const TURN_STEP: u16 = 15;
const TURN_BIG: u16 = 45;
/// How many percent one press of the sensitivity buttons moves, and with Shift.
const SENS_STEP: u16 = 10;
const SENS_BIG: u16 = 25;
const NAME_CHARS: usize = 38;

/// The words the tab says under the rows.
const NOTES: [&str; 4] = [
    "Click a knob, then pinch thumb and index finger and twist your wrist to turn it.",
    "As a MIDI controller the camera is an input named Camera: map its controls with MIDI Learn.",
    "The camera picture stays in memory: it is never saved, logged or sent anywhere.",
    "Saved in workflow.json next to settings.json when you press Apply.",
];

/// The camera row.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraRow {
    /// The whole row.
    pub rect: Rect,
    /// The camera as shown.
    pub name: String,
    /// The button that picks the previous camera.
    pub prev: Rect,
    /// The button that picks the next camera.
    pub next: Rect,
    /// The button that asks the system for the list.
    pub refresh: Rect,
}

/// The mode row.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeRow {
    /// The whole row.
    pub rect: Rect,
    /// The three choices.
    pub choices: Vec<(CameraMode, Rect)>,
}

/// The hand row.
#[derive(Debug, Clone, PartialEq)]
pub struct HandRow {
    /// The whole row.
    pub rect: Rect,
    /// The three choices.
    pub choices: Vec<(HandChoice, Rect)>,
}

/// A row with a number and two step buttons.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRow {
    /// The whole row.
    pub rect: Rect,
    /// The number as shown.
    pub value: String,
    /// The button that takes a step off.
    pub less: Rect,
    /// The box the number is in.
    pub field: Rect,
    /// The button that adds a step.
    pub more: Rect,
}

/// A row with a check box.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckRow {
    /// The whole row.
    pub rect: Rect,
    /// The check box.
    pub check: Rect,
    /// The box is ticked.
    pub on: bool,
}

/// The models row.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelsRow {
    /// The whole row.
    pub rect: Rect,
    /// What the models are like now.
    pub status: &'static str,
    /// **Delete downloaded models**.
    pub delete: Rect,
    /// Whether there is anything to delete.
    pub can_delete: bool,
}

/// Where the tab's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraLayout {
    /// The camera row.
    pub camera: CameraRow,
    /// What the hands do.
    pub mode: ModeRow,
    /// The hand row.
    pub hand: HandRow,
    /// Twist for full range.
    pub turn: StepRow,
    /// Fine control by distance.
    pub fine: CheckRow,
    /// Pinch sensitivity.
    pub sensitivity: StepRow,
    /// Show camera preview.
    pub preview: CheckRow,
    /// Camera notes play the selected instrument.
    pub play_notes: CheckRow,
    /// The models row.
    pub models: ModelsRow,
    /// The notes under the rows, line by line.
    pub notes: Vec<(Point, String)>,
}

/// The Camera tab state.
#[derive(Debug, Clone)]
pub struct CameraTab {
    prefs: CameraPrefs,
    cameras: Vec<CameraEntry>,
    models: ModelState,
    hover: Option<Point>,
}

fn shorten(text: &str) -> String {
    let count = text.chars().count();
    if count <= NAME_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(NAME_CHARS - 1).collect();
    format!("{head}\u{2026}")
}

impl CameraTab {
    /// A tab editing `prefs`, with the models in `models`.
    pub fn new(prefs: CameraPrefs, models: ModelState) -> Self {
        Self {
            prefs,
            cameras: Vec::new(),
            models,
            hover: None,
        }
    }

    /// The edited settings.
    pub fn prefs(&self) -> &CameraPrefs {
        &self.prefs
    }

    /// Shows the cameras the system listed.
    pub fn set_cameras(&mut self, cameras: Vec<CameraEntry>) {
        self.cameras = cameras;
    }

    /// Shows the state of the models (after a delete).
    pub fn set_models(&mut self, models: ModelState) {
        self.models = models;
    }

    /// The camera choice as text: its name when listed, else its id, else Automatic.
    fn camera_name(&self) -> String {
        match &self.prefs.camera {
            None => atomik_ui::i18n::tr("Automatic: the first camera").into_owned(),
            Some(id) => self
                .cameras
                .iter()
                .find(|c| &c.id == id)
                .map_or_else(|| id.clone(), |c| c.name.clone()),
        }
    }

    /// Picks the camera `delta` places along Automatic and the listed cameras.
    fn step_camera(&mut self, delta: i32) {
        let n = self.cameras.len() as i32 + 1;
        let at = match &self.prefs.camera {
            None => 0,
            Some(id) => self
                .cameras
                .iter()
                .position(|c| &c.id == id)
                .map_or(0, |i| i as i32 + 1),
        };
        let next = (at + delta).rem_euclid(n);
        self.prefs.camera = (next > 0).then(|| self.cameras[next as usize - 1].id.clone());
    }

    /// Lays the tab out inside `panel`.
    pub fn layout(&self, panel: Rect) -> CameraLayout {
        let x0 = panel.x0 + PAD;
        let inner = panel.width() - 2.0 * PAD;
        let mut y = panel.y0 + TITLE;
        let mut next_row = || {
            let r = Rect::new(x0, y, x0 + inner, y + ROW);
            y += ROW + GAP;
            r
        };
        let button =
            |r: Rect, right: f64, w: f64| Rect::new(right - w, r.y0 + 6.0, right, r.y0 + 34.0);

        let rect = next_row();
        let refresh = button(rect, rect.x1, 76.0);
        let next = button(rect, refresh.x0 - GAP, STEP_W);
        let prev = button(rect, next.x0 - GAP, STEP_W);
        let camera = CameraRow {
            rect,
            name: shorten(&self.camera_name()),
            prev,
            next,
            refresh,
        };

        let rect = next_row();
        let mut mode_choices = Vec::new();
        let mut right = rect.x1;
        for choice in CameraMode::ALL.into_iter().rev() {
            let r = button(rect, right, MODE_W);
            right = r.x0 - GAP;
            mode_choices.push((choice, r));
        }
        mode_choices.reverse();
        let mode = ModeRow {
            rect,
            choices: mode_choices,
        };

        let rect = next_row();
        let mut choices = Vec::new();
        let mut right = rect.x1;
        for choice in HandChoice::ALL.into_iter().rev() {
            let r = button(rect, right, BUTTON_W);
            right = r.x0 - GAP;
            choices.push((choice, r));
        }
        choices.reverse();
        let hand = HandRow { rect, choices };

        let step_row = |rect: Rect, value: String| {
            let more = button(rect, rect.x1, STEP_W);
            let field = Rect::new(more.x0 - 56.0 - GAP, more.y0, more.x0 - GAP, more.y1);
            let less = button(rect, field.x0 - GAP, STEP_W);
            StepRow {
                rect,
                value,
                less,
                field,
                more,
            }
        };
        let turn = step_row(next_row(), format!("{}\u{b0}", self.prefs.full_turn));
        let check_row = |rect: Rect, on: bool| CheckRow {
            rect,
            check: Rect::new(rect.x0 + 8.0, rect.y0 + 6.0, rect.x0 + 24.0, rect.y0 + 22.0),
            on,
        };
        let fine = check_row(next_row(), self.prefs.fine);
        let sensitivity = step_row(next_row(), format!("{}%", self.prefs.sensitivity));
        let preview = check_row(next_row(), self.prefs.preview);
        let play_notes = check_row(next_row(), self.prefs.play_notes);

        let rect = next_row();
        let delete = button(rect, rect.x1, 196.0);
        let models = ModelsRow {
            rect,
            status: match self.models {
                ModelState::Ready => "The hand models are downloaded",
                ModelState::Missing => "The hand models are not downloaded",
                ModelState::Corrupt => "A hand model file is damaged",
            },
            delete,
            can_delete: self.models != ModelState::Missing,
        };
        let mut ny = y + 12.0;
        let mut notes = Vec::new();
        for note in NOTES {
            for line in wrap(note, crate::video_tab::WRAP + 8) {
                notes.push((Point::new(x0, ny + 12.0), line));
                ny += 16.0;
            }
            ny += 6.0;
        }
        CameraLayout {
            camera,
            mode,
            hand,
            turn,
            fine,
            sensitivity,
            preview,
            play_notes,
            models,
            notes,
        }
    }

    /// The height the panel needs for the rows and the notes of this tab.
    pub fn content_height(&self) -> f64 {
        let notes: f64 = NOTES
            .iter()
            .map(|n| 16.0 * wrap(n, crate::video_tab::WRAP + 8).len() as f64 + 6.0)
            .sum();
        9.0 * ROW + 8.0 * GAP + 12.0 + notes
    }

    /// Handles an event inside `panel`.
    pub(crate) fn handle(&mut self, event: &UiEvent, panel: Rect) -> TabOutcome {
        match event {
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Escape => return TabOutcome::Cancel,
                Key::Enter => return TabOutcome::Apply,
                _ => {}
            },
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                ..
            } => {
                let l = self.layout(panel);
                let big = modifiers.shift;
                let turn = if big { TURN_BIG } else { TURN_STEP };
                let sens = if big { SENS_BIG } else { SENS_STEP };
                if l.camera.refresh.contains(*pos) {
                    return TabOutcome::ListCameras;
                } else if l.camera.prev.contains(*pos) {
                    self.step_camera(-1);
                } else if l.camera.next.contains(*pos) {
                    self.step_camera(1);
                } else if let Some((choice, _)) =
                    l.mode.choices.iter().find(|(_, r)| r.contains(*pos))
                {
                    self.prefs.mode = *choice;
                } else if let Some((choice, _)) =
                    l.hand.choices.iter().find(|(_, r)| r.contains(*pos))
                {
                    self.prefs.hand = *choice;
                } else if l.turn.less.contains(*pos) {
                    self.prefs.full_turn = self
                        .prefs
                        .full_turn
                        .saturating_sub(turn)
                        .clamp(MIN_FULL_TURN, MAX_FULL_TURN);
                } else if l.turn.more.contains(*pos) {
                    self.prefs.full_turn = (self.prefs.full_turn + turn).min(MAX_FULL_TURN);
                } else if l.fine.rect.contains(*pos) {
                    self.prefs.fine = !self.prefs.fine;
                } else if l.sensitivity.less.contains(*pos) {
                    self.prefs.sensitivity = self
                        .prefs
                        .sensitivity
                        .saturating_sub(sens)
                        .clamp(MIN_SENSITIVITY, MAX_SENSITIVITY);
                } else if l.sensitivity.more.contains(*pos) {
                    self.prefs.sensitivity = (self.prefs.sensitivity + sens).min(MAX_SENSITIVITY);
                } else if l.preview.rect.contains(*pos) {
                    self.prefs.preview = !self.prefs.preview;
                } else if l.play_notes.rect.contains(*pos) {
                    self.prefs.play_notes = !self.prefs.play_notes;
                } else if l.models.delete.contains(*pos) && l.models.can_delete {
                    return TabOutcome::DeleteCameraModels;
                }
            }
            _ => {}
        }
        TabOutcome::Open
    }

    /// Paints the tab inside `panel`.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, panel: Rect) {
        let l = self.layout(panel);
        let lit = |r: Rect| self.hover.is_some_and(|p| r.contains(p));
        let button = |canvas: &mut dyn Canvas, r: Rect, label: &str, on: bool, enabled: bool| {
            canvas.fill_with(
                RoundedRect::from_rect(r, 6.0),
                if on || (enabled && lit(r)) {
                    theme.selection
                } else {
                    theme.browser_row
                },
            );
            canvas.text(
                &TextRun::new(
                    label,
                    Point::new(r.center().x, r.y0 + 15.5),
                    12.0,
                    if enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                )
                .align(Align::Center),
            );
        };
        let title = |canvas: &mut dyn Canvas, r: Rect, name: &str, sub: &str| {
            canvas.text(&TextRun::new(
                name,
                Point::new(r.x0 + 36.0, r.y0 + 18.0),
                13.0,
                theme.text,
            ));
            if !sub.is_empty() {
                canvas.text(&TextRun::new(
                    sub,
                    Point::new(r.x0 + 36.0, r.y0 + 36.0),
                    11.0,
                    theme.text_muted,
                ));
            }
        };
        let check = |canvas: &mut dyn Canvas, row: &CheckRow, name: &str, sub: &str| {
            if lit(row.rect) {
                canvas.fill_with(RoundedRect::from_rect(row.rect, 6.0), theme.browser_row);
            }
            let shape = RoundedRect::from_rect(row.check, 3.0);
            if row.on {
                canvas.fill_with(shape, theme.focus);
            }
            canvas.stroke_with(shape, 1.2, theme.text_muted);
            title(canvas, row.rect, name, sub);
        };
        let step = |canvas: &mut dyn Canvas, row: &StepRow, name: &str, sub: &str| {
            title(canvas, row.rect, name, sub);
            button(canvas, row.less, "-", false, true);
            button(canvas, row.more, "+", false, true);
            canvas.fill_with(RoundedRect::from_rect(row.field, 6.0), theme.browser_row);
            canvas.text(
                &TextRun::verbatim(
                    row.value.clone(),
                    Point::new(row.field.center().x, row.field.y0 + 16.5),
                    12.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        };

        let c = &l.camera;
        canvas.text(&TextRun::new(
            "Camera",
            Point::new(c.rect.x0 + 36.0, c.rect.y0 + 18.0),
            13.0,
            theme.text,
        ));
        canvas.text(&TextRun::verbatim(
            c.name.clone(),
            Point::new(c.rect.x0 + 36.0, c.rect.y0 + 36.0),
            11.0,
            theme.text_muted,
        ));
        button(canvas, c.prev, "<", false, true);
        button(canvas, c.next, ">", false, true);
        button(canvas, c.refresh, "Refresh", false, true);

        title(canvas, l.mode.rect, "Mode", "What the hands do");
        for (choice, r) in &l.mode.choices {
            button(canvas, *r, choice.label(), self.prefs.mode == *choice, true);
        }
        title(canvas, l.hand.rect, "Hand", "Which hand turns the knob");
        for (choice, r) in &l.hand.choices {
            button(canvas, *r, choice.label(), self.prefs.hand == *choice, true);
        }
        step(
            canvas,
            &l.turn,
            "Twist for full range",
            "How far the wrist turns to sweep a whole range; Shift moves by 45",
        );
        check(
            canvas,
            &l.fine,
            "Fine control by distance",
            "A hand close to the camera turns the knob finer",
        );
        step(
            canvas,
            &l.sensitivity,
            "Pinch sensitivity",
            "Higher: the fingers may be further apart",
        );
        check(
            canvas,
            &l.preview,
            "Show camera preview",
            "A small box with the camera picture while the camera is on",
        );
        check(
            canvas,
            &l.play_notes,
            "Camera notes play the selected instrument",
            "Off: the camera only feeds mappings, as a controller",
        );
        title(canvas, l.models.rect, "Hand models", l.models.status);
        button(
            canvas,
            l.models.delete,
            "Delete downloaded models",
            false,
            l.models.can_delete,
        );
        for (at, line) in &l.notes {
            canvas.text(&TextRun::new(line.clone(), *at, 11.5, theme.text_muted));
        }
    }
}
