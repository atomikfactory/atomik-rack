//! The practice session in the app core: the speed, the count-in and the speed trainer behind the
//! practice bar of the Tab Editor and Chord Sheet panes (see [`crate::practice`]).
//!
//! The speed is a runtime factor of the engine transport (`TransportCommand::Speed`): it is not
//! saved, not applied to exports and bounces (they run their own engines), and reset to 100 %
//! whenever a project is opened or created. The count-in applies to a plain Play (a recording has
//! its own pre-count). The trainer watches the engine's loop count once a frame
//! ([`AppCore::practice_tick`]) and asks the pass judge about each completed loop cycle.

use super::*;
use crate::practice::{PassJudge, Practice, PracticeAction};
use atomik_ui::PracticeState;

impl<S: EngineSink> AppCore<S> {
    /// The practice session.
    pub fn practice(&self) -> &Practice {
        &self.practice
    }

    /// Replaces the judge that decides which passes of the loop count for the speed trainer
    /// (the default counts every pass).
    pub fn set_pass_judge(&mut self, judge: Box<dyn PassJudge>) {
        self.pass_judge = judge;
    }

    /// What the practice bars show.
    pub(super) fn practice_state(&self) -> PracticeState {
        self.practice
            .view(self.metronome, self.doc().song.loop_enabled)
    }

    /// Runs a practice command.
    pub fn practice_action(&mut self, action: PracticeAction) {
        match action {
            PracticeAction::Speed(percent) => self.set_practice_speed(percent),
            PracticeAction::SpeedUp => {
                let step = atomik_ui::SPEED_STEP_PERCENT;
                self.set_practice_speed(self.practice.speed() + step);
            }
            PracticeAction::SpeedDown => {
                let step = atomik_ui::SPEED_STEP_PERCENT;
                self.set_practice_speed(self.practice.speed().saturating_sub(step));
            }
            PracticeAction::CountIn(bars) => self.practice.set_count_in(bars),
            PracticeAction::ToggleTrainer => {
                if self.practice_blocked() {
                    return;
                }
                if self.practice.toggle_trainer().is_some() {
                    self.send_practice_speed();
                    if !self.doc().song.loop_enabled {
                        self.report(
                            "the speed trainer counts passes of the loop: turn the loop on (Loop Selection)"
                                .to_owned(),
                        );
                    }
                }
                self.last_pass_loops = None;
            }
            PracticeAction::Trainer(value, up) => {
                if self.practice.adjust_trainer(value, up).is_some() {
                    self.send_practice_speed();
                }
            }
            PracticeAction::LoopSelection => {
                let pane = if self.tab_clip.is_some() {
                    Pane::TabEditor
                } else if self.sheet_open {
                    Pane::ChordSheet
                } else {
                    self.report(
                        "open the Tab Editor or the Chord Sheet to loop its selection".to_owned(),
                    );
                    return;
                };
                let _ = self.pane_command(pane, "practice.loop_selection", None);
            }
        }
        self.show_transport();
    }

    /// Whether a speed change must be refused now (and says why).
    fn practice_blocked(&mut self) -> bool {
        if self.sink.sync_status().following {
            self.reject(
                "the transport follows the MIDI clock: switch Options > Sync > Follow MIDI Clock off to change the practice speed"
                    .to_string(),
            );
            return true;
        }
        false
    }

    fn set_practice_speed(&mut self, percent: u16) {
        if self.practice_blocked() {
            return;
        }
        if self.practice.set_speed(percent) {
            self.send_practice_speed();
        }
    }

    /// Sends the speed to the engine.
    pub(super) fn send_practice_speed(&mut self) {
        self.send_transport(TransportCommand::Speed(self.practice.factor()));
    }

    /// The count-in a plain Play would have: the practice bars at the position the transport
    /// stands at. `None` when it is off, the transport already runs, a take is recording (its
    /// own pre-count applies) or there is no song to count tempo and meter from.
    fn practice_count_in(&self) -> Option<PreRoll> {
        let bars = self.practice.count_in();
        if self.playing || bars == 0 || self.take.is_some() {
            return None;
        }
        let rate = f64::from(self.doc().sample_rate);
        self.snapshot
            .as_ref()?
            .pre_roll(Ticks(self.position.max(0)), bars, rate)
    }

    /// Play, after the practice count-in when it is on.
    pub(super) fn play_counted(&mut self) {
        let plan = self.practice_count_in();
        self.play_from(plan);
    }

    /// Once a frame: counts the loop cycles the engine completed since the last frame for the
    /// speed trainer and sends the speed it steps to.
    pub(super) fn practice_tick(&mut self) {
        if !self.practice.trainer().enabled || !self.playing {
            self.last_pass_loops = None;
            return;
        }
        let loops = self.sink.transport_state().loop_count;
        let Some(last) = self.last_pass_loops.replace(loops) else {
            return;
        };
        // A new engine counts from zero again.
        let passes = loops.saturating_sub(last);
        let mut stepped = false;
        for _ in 0..passes {
            if self.practice.pass(self.pass_judge.as_mut()).is_some() {
                stepped = true;
            }
        }
        if stepped {
            self.send_practice_speed();
        }
        if passes > 0 {
            self.show_transport();
        }
    }
}
