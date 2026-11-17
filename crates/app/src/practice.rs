//! The practice session: the speed, the count-in and the speed trainer of the Tab Editor and
//! Chord Sheet panes' practice bar (see [`atomik_ui::PracticeBar`]).
//!
//! Nothing here is part of the project. The speed is a runtime factor of the engine transport
//! (`TransportCommand::Speed`), which the app resets to 100 % whenever a project is opened or
//! created; the count-in and the trainer's values live as long as the app runs.
//!
//! The **trainer** raises the speed by `step` percent each time `after` clean passes of the loop
//! have been played at the current speed, from `start` up to `target`. A *pass* is one completed
//! loop cycle; whether it was *clean* is for a [`PassJudge`] to say. The default judge,
//! [`EveryPass`], counts every pass; the listen-and-grade feature replaces it with one that asks
//! for an accuracy threshold ([`crate::AppCore::set_pass_judge`]).

use atomik_ui::{
    MAX_SPEED_PERCENT, MIN_SPEED_PERCENT, PracticeState, SPEED_STEP_PERCENT, TrainerState,
    TrainerValue,
};

/// Most bars of count-in a plain playback takes.
pub const MAX_COUNT_IN_BARS: u8 = 2;
/// The most clean passes a trainer step may ask for.
pub const MAX_TRAINER_PASSES: u8 = 9;
/// The biggest step of the trainer, in percent.
pub const MAX_TRAINER_STEP: u16 = 25;

/// A command of the practice bar, menu and command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PracticeAction {
    /// Set the speed to this many percent (a multiple of 5 from 25 to 150).
    Speed(u16),
    /// One step faster.
    SpeedUp,
    /// One step slower.
    SpeedDown,
    /// The count-in length in bars (0 = off).
    CountIn(u8),
    /// The speed trainer on or off.
    ToggleTrainer,
    /// A value of the trainer up (`true`) or down.
    Trainer(TrainerValue, bool),
    /// Listen and grade on or off.
    ToggleListen,
    /// The next audio input to listen to.
    ListenInput,
    /// Calibrate the listening latency against the click.
    ListenCalibrate,
    /// Clear the results of the session.
    ListenClear,
    /// The manual latency offset up (`true`) or down, by 5 ms.
    ListenOffset(bool),
    /// The accuracy a clean pass needs up (`true`) or down, by 5 points.
    ListenThreshold(bool),
    /// Loop the bars of the selection of the open Tab Editor or Chord Sheet.
    LoopSelection,
}

/// A completed pass of the loop, as the judge sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassReport {
    /// Which pass this is since the trainer was switched on (1 for the first).
    pub index: u32,
    /// The speed it was played at, in percent.
    pub speed: u16,
    /// How accurately it was played, as a fraction, when the pass was graded from the audio
    /// input (listening on); `None` otherwise.
    pub accuracy: Option<f32>,
}

/// Decides whether a completed pass counts towards the trainer's next step.
pub trait PassJudge: Send {
    /// True when `pass` was clean.
    fn is_clean(&mut self, pass: &PassReport) -> bool;
}

/// The default judge: every pass counts.
#[derive(Debug, Clone, Copy, Default)]
pub struct EveryPass;

impl PassJudge for EveryPass {
    fn is_clean(&mut self, _: &PassReport) -> bool {
        true
    }
}

/// The practice session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Practice {
    speed: u16,
    count_in: u8,
    trainer: TrainerState,
}

impl Default for Practice {
    fn default() -> Self {
        Self {
            speed: 100,
            count_in: 0,
            trainer: TrainerState::default(),
        }
    }
}

fn step_value(value: u16, up: bool, step: u16, (lo, hi): (u16, u16)) -> u16 {
    if up {
        value.saturating_add(step).min(hi)
    } else {
        value.saturating_sub(step).max(lo)
    }
}

impl Practice {
    /// The speed in percent.
    pub fn speed(&self) -> u16 {
        self.speed
    }

    /// The count-in in bars.
    pub fn count_in(&self) -> u8 {
        self.count_in
    }

    /// The trainer.
    pub fn trainer(&self) -> TrainerState {
        self.trainer
    }

    /// What the practice bar shows (without the listening part, which the app core adds).
    pub fn view(&self, metronome: bool, looping: bool) -> PracticeState {
        PracticeState {
            speed: self.speed,
            count_in: self.count_in,
            metronome,
            looping,
            listen: atomik_ui::ListenState::default(),
            trainer: self.trainer,
        }
    }

    /// Sets the speed (clamped to 25..=150 and rounded to a step of 5); false when it did not
    /// change.
    pub fn set_speed(&mut self, percent: u16) -> bool {
        let step = SPEED_STEP_PERCENT;
        let v = ((percent + step / 2) / step * step).clamp(MIN_SPEED_PERCENT, MAX_SPEED_PERCENT);
        let changed = v != self.speed;
        self.speed = v;
        changed
    }

    /// Sets the count-in length (at most two bars).
    pub fn set_count_in(&mut self, bars: u8) {
        self.count_in = bars.min(MAX_COUNT_IN_BARS);
    }

    /// Switches the trainer. Switching it on starts it: the speed becomes its start speed and the
    /// pass count begins afresh (the speed to set is returned); switching it off leaves the speed
    /// where it is.
    pub fn toggle_trainer(&mut self) -> Option<u16> {
        self.trainer.enabled = !self.trainer.enabled;
        self.trainer.passes = 0;
        self.trainer.clean = 0;
        self.trainer.enabled.then(|| {
            self.speed = self.trainer.start;
            self.speed
        })
    }

    /// Edits a value of the trainer. The target never goes under the start speed nor the start
    /// over the target. While the trainer runs, a new start speed restarts it (the speed to set
    /// is returned).
    pub fn adjust_trainer(&mut self, value: TrainerValue, up: bool) -> Option<u16> {
        let t = &mut self.trainer;
        let speeds = (MIN_SPEED_PERCENT, MAX_SPEED_PERCENT);
        match value {
            TrainerValue::Start => {
                t.start = step_value(t.start, up, SPEED_STEP_PERCENT, speeds).min(t.target);
            }
            TrainerValue::Target => {
                t.target = step_value(t.target, up, SPEED_STEP_PERCENT, speeds).max(t.start);
            }
            TrainerValue::Step => {
                t.step = step_value(
                    t.step,
                    up,
                    SPEED_STEP_PERCENT,
                    (SPEED_STEP_PERCENT, MAX_TRAINER_STEP),
                );
            }
            TrainerValue::After => {
                t.after = step_value(
                    u16::from(t.after),
                    up,
                    1,
                    (1, u16::from(MAX_TRAINER_PASSES)),
                ) as u8;
            }
        }
        t.clean = 0;
        if value == TrainerValue::Start && t.enabled {
            t.passes = 0;
            self.speed = t.start;
            return Some(self.speed);
        }
        None
    }

    /// A pass of the loop was completed at the current speed. Returns the new speed when it
    /// made the trainer step up. Does nothing while the trainer is off.
    pub fn pass(&mut self, judge: &mut dyn PassJudge) -> Option<u16> {
        self.pass_graded(judge, None)
    }

    /// [`Self::pass`] for a pass graded from the audio input (`accuracy` as a fraction).
    pub fn pass_graded(&mut self, judge: &mut dyn PassJudge, accuracy: Option<f32>) -> Option<u16> {
        if !self.trainer.enabled {
            return None;
        }
        self.trainer.passes += 1;
        let report = PassReport {
            index: self.trainer.passes,
            speed: self.speed,
            accuracy,
        };
        if !judge.is_clean(&report) {
            return None;
        }
        if self.speed >= self.trainer.target {
            // At the target the trainer has nothing left to raise.
            return None;
        }
        self.trainer.clean += 1;
        if self.trainer.clean < self.trainer.after {
            return None;
        }
        self.trainer.clean = 0;
        self.speed = (self.speed + self.trainer.step).min(self.trainer.target);
        Some(self.speed)
    }

    /// A project was opened or created: back to 100 % and the trainer off.
    pub fn reset_for_project(&mut self) {
        self.speed = 100;
        self.trainer.enabled = false;
        self.trainer.passes = 0;
        self.trainer.clean = 0;
    }

    /// The speed as the engine factor.
    pub fn factor(&self) -> f32 {
        f32::from(self.speed) / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Alternate(bool);

    impl PassJudge for Alternate {
        fn is_clean(&mut self, _: &PassReport) -> bool {
            self.0 = !self.0;
            self.0
        }
    }

    #[test]
    fn the_speed_is_clamped_and_rounded_to_five() {
        let mut p = Practice::default();
        assert!(!p.set_speed(100));
        assert!(p.set_speed(12));
        assert_eq!(p.speed(), 25);
        p.set_speed(77);
        assert_eq!(p.speed(), 75);
        p.set_speed(500);
        assert_eq!(p.speed(), 150);
        assert!((p.factor() - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn the_trainer_starts_at_its_start_speed_and_climbs_after_clean_passes() {
        let mut p = Practice::default();
        assert_eq!(p.toggle_trainer(), Some(50));
        let mut judge = EveryPass;
        let mut speeds = Vec::new();
        for _ in 0..13 {
            p.pass(&mut judge);
            speeds.push(p.speed());
        }
        // Two passes per step of five, up to the target of 100: ten steps in twenty passes.
        assert_eq!(speeds, [50, 55, 55, 60, 60, 65, 65, 70, 70, 75, 75, 80, 80]);
        for _ in 0..40 {
            p.pass(&mut judge);
        }
        assert_eq!(p.speed(), 100, "the trainer stops at the target");
        assert_eq!(p.trainer().passes, 53);
    }

    #[test]
    fn only_clean_passes_count_and_a_dirty_one_does_not_reset_the_streak() {
        let mut p = Practice::default();
        p.toggle_trainer();
        let mut judge = Alternate(false);
        // Clean, dirty, clean, dirty ...: two clean passes need four.
        let mut ups = Vec::new();
        for i in 0..4 {
            if p.pass(&mut judge).is_some() {
                ups.push(i);
            }
        }
        assert_eq!(ups, [2]);
        assert_eq!(p.trainer().passes, 4);
    }

    #[test]
    fn passes_are_not_counted_while_the_trainer_is_off() {
        let mut p = Practice::default();
        assert_eq!(p.pass(&mut EveryPass), None);
        assert_eq!(p.trainer().passes, 0);
        p.toggle_trainer();
        p.pass(&mut EveryPass);
        assert_eq!(p.toggle_trainer(), None, "off again leaves the speed");
        assert_eq!(p.speed(), 50);
        assert_eq!(p.trainer().passes, 0);
    }

    #[test]
    fn trainer_values_keep_their_order_and_limits() {
        let mut p = Practice::default();
        for _ in 0..30 {
            p.adjust_trainer(TrainerValue::Start, true);
        }
        assert_eq!(p.trainer().start, 100, "the start stops at the target");
        for _ in 0..30 {
            p.adjust_trainer(TrainerValue::Target, false);
        }
        assert_eq!(p.trainer().target, 100);
        for _ in 0..30 {
            p.adjust_trainer(TrainerValue::Target, true);
            p.adjust_trainer(TrainerValue::Step, true);
            p.adjust_trainer(TrainerValue::After, true);
        }
        let t = p.trainer();
        assert_eq!(
            (t.target, t.step, t.after),
            (150, MAX_TRAINER_STEP, MAX_TRAINER_PASSES)
        );
        for _ in 0..30 {
            p.adjust_trainer(TrainerValue::Step, false);
            p.adjust_trainer(TrainerValue::After, false);
        }
        assert_eq!((p.trainer().step, p.trainer().after), (5, 1));
    }

    #[test]
    fn a_new_start_speed_restarts_a_running_trainer() {
        let mut p = Practice::default();
        p.toggle_trainer();
        p.pass(&mut EveryPass);
        assert_eq!(p.adjust_trainer(TrainerValue::Start, true), Some(55));
        assert_eq!(
            (p.speed(), p.trainer().passes, p.trainer().clean),
            (55, 0, 0)
        );
        assert_eq!(p.adjust_trainer(TrainerValue::Step, true), None);
    }

    #[test]
    fn a_project_resets_the_speed_and_the_trainer_but_not_the_count_in() {
        let mut p = Practice::default();
        p.set_count_in(9);
        assert_eq!(p.count_in(), 2);
        p.toggle_trainer();
        p.reset_for_project();
        assert_eq!((p.speed(), p.count_in()), (100, 2));
        assert!(!p.trainer().enabled);
    }
}
