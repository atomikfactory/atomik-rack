//! The set-up wizard of a controller, as a state machine with no I/O: it is told the messages of
//! the chosen input port and asks for one control after another ("Move knob 1", "Move knob 2",
//! ..., "Press Play"). A control the keyboard lacks is skipped; the controls it did hear become a
//! [`ControllerProfile`].
//!
//! The wizard moves on by itself as soon as a control is known ([`Progress::Accepted`]); the
//! controls already set up are not heard again, so a knob that is still turning does not answer
//! the next question. Back takes the last answer back. When there is an output port the wizard
//! asks, per group (the knobs, the faders, the keys), whether the controller shows values back
//! (LED rings, motorised faders, lit keys); the answer goes into the profile. On the last page
//! the two switches of a profile can be turned over: pick up values, keep the knob page per
//! device ([`Wizard::set_pickup`], [`Wizard::set_keep_page`]).
//!
//! The shell owns the port and the screen ([`crate::controller_wizard_view`]); everything that
//! decides something is here and is tested from fake message streams.

use crate::controller_detect::{Capture, Detection, Verdict, Want};
use crate::controller_profile::{
    ControlSpec, ControllerProfile, FADERS, KNOBS, ProfileOptions, Role,
};
use atomik_core::ChannelMessage;

/// A group of controls the feedback question is asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// The knobs.
    Knobs,
    /// The faders (and the master fader).
    Faders,
    /// The transport keys.
    Keys,
}

impl Group {
    fn of(role: Role) -> Option<Group> {
        match role {
            Role::Knob(_) => Some(Group::Knobs),
            Role::Fader(_) | Role::MasterFader => Some(Group::Faders),
            Role::Play | Role::Stop | Role::Record | Role::Loop => Some(Group::Keys),
            Role::BankLeft | Role::BankRight | Role::PageDown | Role::PageUp => None,
        }
    }

    /// The question the screen asks.
    pub fn question(self) -> &'static str {
        match self {
            Group::Knobs => "Do the knobs show their value (LED rings)?",
            Group::Faders => "Do the faders move by themselves (motorised)?",
            Group::Keys => "Do the transport keys light up?",
        }
    }

    const ALL: [Group; 3] = [Group::Knobs, Group::Faders, Group::Keys];
}

/// Where the wizard is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Waiting for this control.
    Control(Role),
    /// Asking whether this group shows values.
    Ask(Group),
    /// Finished: [`Wizard::profile`] has the result.
    Done,
}

/// What a message did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Nothing to show.
    Idle,
    /// A control is being heard but not known yet ([`Wizard::hint`] says what to do).
    Hearing,
    /// The control was learned; the wizard has moved on.
    Accepted(ControlSpec),
}

/// One answered step, kept so that Back can undo it.
#[derive(Debug, Clone, Copy)]
struct Entry {
    role: Role,
    spec: Option<ControlSpec>,
}

/// The wizard.
#[derive(Debug, Clone)]
pub struct Wizard {
    input: String,
    output: Option<String>,
    roles: Vec<Role>,
    /// The step now: an index into `roles`; `roles.len()` means the questions.
    at: usize,
    history: Vec<Entry>,
    capture: Capture,
    hint: Option<&'static str>,
    heard: Option<Detection>,
    /// The groups still to ask about, front first.
    asks: Vec<Group>,
    /// The groups answered yes.
    shows: Vec<Group>,
    options: ProfileOptions,
    done: bool,
}

/// The controls the wizard asks for, in order.
pub fn steps() -> Vec<Role> {
    let mut v: Vec<Role> = (1..=KNOBS).map(Role::Knob).collect();
    v.extend((1..=FADERS).map(Role::Fader));
    v.extend([
        Role::MasterFader,
        Role::Play,
        Role::Stop,
        Role::Record,
        Role::Loop,
        Role::BankLeft,
        Role::BankRight,
        Role::PageDown,
        Role::PageUp,
    ]);
    v
}

impl Wizard {
    /// A wizard for the controller on `input`; `output` is the port that carries the feedback
    /// (`None` when the controller has none: the questions are not asked).
    pub fn new(input: &str, output: Option<&str>) -> Self {
        let mut w = Self {
            input: input.to_owned(),
            output: output.map(str::to_owned),
            roles: steps(),
            at: 0,
            history: Vec::new(),
            capture: Capture::new(Want::Continuous, Vec::new()),
            hint: None,
            heard: None,
            asks: Vec::new(),
            shows: Vec::new(),
            options: ProfileOptions::default(),
            done: false,
        };
        w.renew();
        w
    }

    /// The input port.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The output port, when there is one.
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Where the wizard is.
    pub fn stage(&self) -> Stage {
        if self.done {
            Stage::Done
        } else if let Some(role) = self.roles.get(self.at) {
            Stage::Control(*role)
        } else {
            self.asks.first().map_or(Stage::Done, |g| Stage::Ask(*g))
        }
    }

    /// The instruction for the screen (`Move knob 3`, `Press Play`).
    pub fn prompt(&self) -> String {
        match self.stage() {
            Stage::Control(role) => match role {
                Role::Knob(n) => format!("Move knob {n}"),
                Role::Fader(n) => format!("Move fader {n}"),
                Role::MasterFader => "Move the master fader".to_owned(),
                Role::Play => "Press Play".to_owned(),
                Role::Stop => "Press Stop".to_owned(),
                Role::Record => "Press Record".to_owned(),
                Role::Loop => "Press Loop (cycle)".to_owned(),
                Role::BankLeft => "Press Bank Left (previous strips)".to_owned(),
                Role::BankRight => "Press Bank Right (next strips)".to_owned(),
                Role::PageDown => "Press Page Down (previous knobs)".to_owned(),
                Role::PageUp => "Press Page Up (next knobs)".to_owned(),
            },
            Stage::Ask(group) => group.question().to_owned(),
            Stage::Done => "Done".to_owned(),
        }
    }

    /// The switches the profile will get.
    pub fn options(&self) -> ProfileOptions {
        self.options
    }

    /// Turns "pick up values" on or off.
    pub fn set_pickup(&mut self, on: bool) {
        self.options.pickup = on;
    }

    /// Turns "keep the knob page per device" on or off.
    pub fn set_keep_page(&mut self, on: bool) {
        self.options.keep_page = on;
    }

    /// How far the wizard is: `(controls answered, controls asked)`.
    pub fn position(&self) -> (usize, usize) {
        (self.at.min(self.roles.len()), self.roles.len())
    }

    /// What to do next when a control was heard but is not known yet.
    pub fn hint(&self) -> Option<&'static str> {
        self.hint
    }

    /// What the last message looked like so far (`CC 21 on channel 1, absolute`), while the
    /// control is not accepted yet.
    pub fn heard(&self) -> Option<Detection> {
        self.heard
    }

    /// The controls learned so far.
    pub fn learned(&self) -> Vec<ControlSpec> {
        self.history.iter().filter_map(|e| e.spec).collect()
    }

    /// The controls learned so far, with how many are keys, knobs and faders.
    pub fn count(&self) -> usize {
        self.history.iter().filter(|e| e.spec.is_some()).count()
    }

    fn want_of(role: Role) -> Want {
        if role.is_button() {
            Want::Button
        } else {
            Want::Continuous
        }
    }

    /// A new capture for the step now (the learned controls are ignored).
    fn renew(&mut self) {
        let want = self
            .roles
            .get(self.at)
            .map_or(Want::Continuous, |r| Self::want_of(*r));
        let ignore = self
            .learned()
            .iter()
            .map(|c| (c.source, c.channel))
            .collect();
        self.capture = Capture::new(want, ignore);
        self.hint = None;
        self.heard = None;
    }

    /// Takes one message of the input port.
    pub fn feed(&mut self, msg: &ChannelMessage) -> Progress {
        let Stage::Control(role) = self.stage() else {
            return Progress::Idle;
        };
        self.capture.feed(msg);
        match self.capture.verdict() {
            Verdict::Nothing => Progress::Idle,
            Verdict::NeedsMore(hint) => {
                self.hint = Some(hint);
                Progress::Hearing
            }
            Verdict::Found(detection) => {
                self.heard = Some(detection);
                let spec = ControlSpec {
                    role,
                    source: detection.source,
                    channel: detection.channel,
                    mode: detection.mode,
                    feedback: false,
                };
                self.history.push(Entry {
                    role,
                    spec: Some(spec),
                });
                self.advance();
                Progress::Accepted(spec)
            }
        }
    }

    /// Leaves the control the controller does not have.
    pub fn skip(&mut self) {
        if let Stage::Control(role) = self.stage() {
            self.history.push(Entry { role, spec: None });
            self.advance();
        }
    }

    /// Leaves this control and the rest of its kind (the knobs, or the faders; a key alone).
    pub fn skip_group(&mut self) {
        let class = |r: Role| match r {
            Role::Knob(_) => 0,
            Role::Fader(_) | Role::MasterFader => 1,
            other => 2 + steps().iter().position(|s| *s == other).unwrap_or(0),
        };
        let Stage::Control(first) = self.stage() else {
            return;
        };
        while let Stage::Control(role) = self.stage() {
            if class(role) != class(first) {
                break;
            }
            self.history.push(Entry { role, spec: None });
            self.advance();
        }
    }

    /// Finishes now with what was learned.
    pub fn finish_now(&mut self) {
        self.at = self.roles.len();
        self.begin_questions();
    }

    /// Takes the last answer back (the control is asked for again).
    pub fn back(&mut self) {
        if let Some(last) = self.history.pop() {
            if let Some(i) = self.roles.iter().position(|r| *r == last.role) {
                self.at = i;
            }
            self.done = false;
            self.asks.clear();
            self.shows.clear();
            self.renew();
        }
    }

    /// Answers the question on the screen.
    pub fn answer(&mut self, yes: bool) {
        if let Stage::Ask(group) = self.stage() {
            if yes {
                self.shows.push(group);
            }
            self.asks.remove(0);
            if self.asks.is_empty() {
                self.done = true;
            }
        }
    }

    fn advance(&mut self) {
        self.at += 1;
        if self.at >= self.roles.len() {
            self.begin_questions();
        } else {
            self.renew();
        }
    }

    fn begin_questions(&mut self) {
        self.at = self.roles.len();
        let learned = self.learned();
        self.asks = if self.output.is_some() {
            Group::ALL
                .into_iter()
                .filter(|g| learned.iter().any(|c| Group::of(c.role) == Some(*g)))
                .collect()
        } else {
            Vec::new()
        };
        self.shows.clear();
        self.done = self.asks.is_empty();
        self.hint = None;
        self.heard = None;
    }

    /// The profile of what was learned, under `name`, once the wizard is [`Stage::Done`].
    pub fn profile(&self, name: &str) -> Option<ControllerProfile> {
        if self.stage() != Stage::Done {
            return None;
        }
        let controls: Vec<ControlSpec> = self
            .learned()
            .into_iter()
            .map(|mut c| {
                c.feedback = Group::of(c.role).is_some_and(|g| self.shows.contains(&g));
                c
            })
            .collect();
        (!controls.is_empty()).then(|| ControllerProfile {
            name: name.to_owned(),
            input: self.input.clone(),
            output: self.output.clone(),
            controls,
            options: self.options,
        })
    }
}

#[cfg(test)]
#[path = "controller_wizard_tests.rs"]
mod tests;
