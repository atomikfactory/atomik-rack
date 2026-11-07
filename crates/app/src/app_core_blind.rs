//! The loudness match switch and readout and the blind A/B test in the app core.
//!
//! * **The switch.** `internal.match` of an effect is monitoring state like the EQ analyser
//!   switch: set on the engine from a per-device setting kept here ([`AppCore::toggle_match`]),
//!   never stored in the project, not an undo step, and so never part of a bounce or an export
//!   (they render the document). The setting is sent when it changes, again after an engine
//!   restart and for a device that was added anew, and dropped with its device.
//! * **The readout.** An effect whose match is on publishes the gain it applies after its own
//!   meter values ([`atomik_devices::bypass::match_gain_of`]). Once per frame
//!   [`AppCore::feed_match_gains`] reads it for every such effect and gives the rack view the
//!   gain; an effect whose match went off is told so. The gain is read from the engine.
//! * **The blind test.** [`AppCore::blind_start`] takes one effect and runs a
//!   [`BlindTest`](crate::blind_ab::BlindTest) on it. The effect's `bypass` and, for the test,
//!   its match are set straight on the engine: the document is never touched, so there is
//!   nothing to undo, and when the test ends (the last trial, the panel closing, the device
//!   going away) the engine gets the document's `bypass` and the match setting back. A new
//!   engine starts from the document, so a restart simply ends the test.

use super::*;
use crate::blind_ab::{BlindTest, DEFAULT_TRIALS};
use atomik_core::ParamId;
use atomik_devices::bypass::{MATCH_KEY, match_gain_of};
use atomik_ui::{BlindAction, BlindInfo};
use std::collections::{BTreeMap, BTreeSet};

/// A running blind test and the device it works on.
#[derive(Debug)]
struct Session {
    test: BlindTest,
    device: DeviceKey,
    id: DeviceInstanceId,
    name: String,
    bypass: ParamId,
    /// The match is on while the test plays.
    match_on: bool,
    /// The device is in the test's hands (false once the test is over and it was restored).
    held: bool,
}

/// The loudness match settings, the blind test and the effects whose readout is showing.
#[derive(Debug, Default)]
pub(super) struct Blind {
    session: Option<Session>,
    shown: BTreeSet<DeviceKey>,
    /// The effects whose loudness match the user switched on.
    on: BTreeSet<DeviceKey>,
    /// `internal.match` as the engine has it: the device's engine id and the value.
    sent: BTreeMap<DeviceKey, (DeviceInstanceId, bool)>,
}

impl Blind {
    /// A new engine knows nothing of the test or of what was sent: the test ends (the new
    /// engine has the document's values).
    pub(super) fn restart(&mut self) {
        self.session = None;
        self.sent.clear();
    }
}

impl<S: EngineSink> AppCore<S> {
    /// The `internal.match` param of the effect `device`: `None` when it is not an effect with
    /// the loudness match.
    fn match_param(&self, device: &DeviceKey) -> Option<ParamId> {
        let dev = self.doc().resolve(device)?;
        self.registry
            .get(&dev.type_id)?
            .descriptor
            .param_by_key(MATCH_KEY)
            .map(|p| p.id)
    }

    /// Whether `device` is an effect with the loudness match.
    pub fn has_match(&self, device: &DeviceKey) -> bool {
        self.match_param(device).is_some()
    }

    /// Whether the loudness match of `device` is on.
    pub fn match_on(&self, device: &DeviceKey) -> bool {
        self.blind.on.contains(device)
    }

    /// Switches the loudness match of the effects among `devices` (all to the opposite of the
    /// first one's setting) and says what it did. Not an undo step: the match is a listening
    /// aid, not part of the project. A device without the switch is skipped.
    pub fn toggle_match(&mut self, devices: &[DeviceKey]) {
        let targets: Vec<DeviceKey> = devices
            .iter()
            .filter(|k| self.has_match(k))
            .cloned()
            .collect();
        let Some(first) = targets.first() else {
            self.report("Select an effect to match its loudness");
            return;
        };
        let on = !self.match_on(first);
        let count = targets.len();
        for key in targets {
            if on {
                self.blind.on.insert(key);
            } else {
                self.blind.on.remove(&key);
            }
        }
        self.push_match();
        self.report(format!(
            "Loudness match {} for {count} effect{}",
            if on { "on" } else { "off" },
            if count == 1 { "" } else { "s" }
        ));
    }

    /// Sends `internal.match` to the effects whose value the engine does not have yet (module
    /// docs): the setting, or the test's own choice for the device under test.
    pub(super) fn push_match(&mut self) {
        // Settings of devices that left the project go with them.
        let gone: Vec<DeviceKey> = self
            .blind
            .on
            .iter()
            .filter(|k| self.doc().resolve(k).is_none())
            .cloned()
            .collect();
        for key in gone {
            self.blind.on.remove(&key);
        }
        let held = self
            .blind
            .session
            .as_ref()
            .filter(|s| s.held)
            .map(|s| (s.device.clone(), s.match_on));
        let mut wanted: BTreeMap<DeviceKey, bool> =
            self.blind.on.iter().map(|k| (k.clone(), true)).collect();
        for key in self.blind.sent.keys() {
            wanted.entry(key.clone()).or_insert(false);
        }
        if let Some((key, on)) = held {
            wanted.insert(key, on);
        }
        for (key, on) in wanted {
            let Some(id) = self.ids.get(&key).copied() else {
                self.blind.sent.remove(&key);
                continue;
            };
            if self.blind.sent.get(&key) == Some(&(id, on)) {
                continue;
            }
            // An engine that never heard of the device already has it off.
            if !on && !self.blind.sent.contains_key(&key) {
                continue;
            }
            let Some(pid) = self.match_param(&key) else {
                continue;
            };
            let result = self.sink.set_param(id, pid, if on { 1.0 } else { 0.0 });
            self.engine_result(result);
            self.blind.sent.insert(key, (id, on));
        }
    }

    /// Starts a blind A/B test of the effect `device` with `trials` trials (the default when 0);
    /// says why when it cannot. A running test ends first.
    pub fn blind_start(&mut self, device: &DeviceKey, trials: usize) -> Result<(), String> {
        self.blind_start_seeded(device, trials, clock_entropy())
    }

    /// [`Self::blind_start`] with the order of the trials fixed by `seed`.
    pub fn blind_start_seeded(
        &mut self,
        device: &DeviceKey,
        trials: usize,
        seed: u64,
    ) -> Result<(), String> {
        self.blind_end();
        let trials = if trials == 0 { DEFAULT_TRIALS } else { trials };
        let Some(dev) = self.doc().resolve(device) else {
            return Err("Select an effect to test first".into());
        };
        let Some(desc) = self.registry.get(&dev.type_id).map(|f| f.descriptor) else {
            return Err("Select an effect to test first".into());
        };
        let (Some(bypass), true) = (
            desc.param_by_key("bypass").map(|p| p.id),
            desc.param_by_key(MATCH_KEY).is_some(),
        ) else {
            return Err(format!("{} has no bypass to compare", desc.name));
        };
        let Some(id) = self.ids.get(device).copied() else {
            return Err("The effect is not running: turn the audio engine on first".into());
        };
        let name = dev.name.clone();
        let match_on = self.match_on(device);
        self.blind.session = Some(Session {
            test: BlindTest::new(trials, seed),
            device: device.clone(),
            id,
            name,
            bypass,
            match_on,
            held: true,
        });
        self.report(format!(
            "Blind A/B started: play the project, listen to X and Y, then pick one ({trials} trials)"
        ));
        Ok(())
    }

    /// Plays slot `slot` (0 = X, 1 = Y) of the running test.
    pub fn blind_listen(&mut self, slot: u8) {
        let Some(s) = &mut self.blind.session else {
            return;
        };
        let Some(processed) = s.test.listen(slot) else {
            return;
        };
        let (id, bypass) = (s.id, s.bypass);
        let result = self
            .sink
            .set_param(id, bypass, if processed { 0.0 } else { 1.0 });
        self.engine_result(result);
        self.push_match();
    }

    /// Judges the current trial for slot `slot`; the last pick ends the test, restores the device
    /// and reports the result.
    pub fn blind_pick(&mut self, slot: u8) {
        let Some(s) = &mut self.blind.session else {
            return;
        };
        if !s.test.pick(slot) {
            return;
        }
        if s.test.finished() {
            self.blind_restore();
            if let Some(score) = self.blind.session.as_ref().and_then(|s| s.test.score()) {
                self.report(format!(
                    "Blind A/B done: effect on picked {} of {} (chance {:.1} %)",
                    score.processed,
                    score.trials,
                    score.p * 100.0
                ));
            }
        }
    }

    /// Switches the loudness match of the test (for the sounds played from now on).
    pub fn blind_toggle_match(&mut self) {
        let Some(s) = &mut self.blind.session else {
            return;
        };
        s.match_on = !s.match_on;
        self.push_match();
    }

    /// Starts the test over on the same device with the same number of trials.
    pub fn blind_again(&mut self) {
        let Some((device, trials)) = self
            .blind
            .session
            .as_ref()
            .map(|s| (s.device.clone(), s.test.trials()))
        else {
            return;
        };
        if let Err(message) = self.blind_start(&device, trials) {
            self.report(message);
        }
    }

    /// Ends the test, whatever its state, and gives the device back as the document has it.
    pub fn blind_end(&mut self) {
        self.blind_restore();
        self.blind.session = None;
    }

    /// Gives the engine the document's `bypass` and the match setting of the tested device, once.
    fn blind_restore(&mut self) {
        let Some(s) = &mut self.blind.session else {
            return;
        };
        if !std::mem::take(&mut s.held) {
            return;
        }
        let (device, id, bypass) = (s.device.clone(), s.id, s.bypass);
        if self.ids.get(&device) != Some(&id) {
            return;
        }
        let value = self
            .doc()
            .param_value(&device, "bypass", &self.registry)
            .unwrap_or(0.0);
        let result = self.sink.set_param(id, bypass, value);
        self.engine_result(result);
        self.push_match();
    }

    /// Whether a blind test is open (running or showing its result).
    pub fn blind_open(&self) -> bool {
        self.blind.session.is_some()
    }

    /// What the blind A/B panel shows; `None` when no test is open.
    pub fn blind_info(&self) -> Option<BlindInfo> {
        let s = self.blind.session.as_ref()?;
        Some(BlindInfo {
            device: s.name.clone(),
            trial: s.test.trial(),
            trials: s.test.trials(),
            listening: s.test.listening(),
            can_pick: s.test.can_pick(),
            match_on: s.match_on,
            result: s.test.score().map(|score| score.lines(&s.name)),
        })
    }

    /// Carries out what the blind A/B panel asked for (`Close` ends the test).
    pub fn blind_action(&mut self, action: BlindAction) {
        match action {
            BlindAction::Listen(slot) => self.blind_listen(slot),
            BlindAction::Pick(slot) => self.blind_pick(slot),
            BlindAction::ToggleMatch => self.blind_toggle_match(),
            BlindAction::Again => self.blind_again(),
            BlindAction::Close => self.blind_end(),
        }
    }

    /// Once per frame: ends a test whose device left the rack, keeps the engine's match switches
    /// current and feeds the match readout.
    pub(super) fn feed_match_gains(&mut self) {
        let gone = self
            .blind
            .session
            .as_ref()
            .is_some_and(|s| self.ids.get(&s.device) != Some(&s.id));
        if gone {
            self.blind.session = None;
        }
        self.push_match();
        let mut buf = std::mem::take(&mut self.fx_meter_buf);
        let mut now: Vec<DeviceKey> = Vec::new();
        for (key, (id, on)) in &self.blind.sent {
            if !*on || self.ids.get(key) != Some(id) {
                continue;
            }
            let gain = self
                .sink
                .read_device_meter_values(*id, &mut buf)
                .and_then(|(n, _)| match_gain_of(&buf[..n]))
                .map_or(0.0, |(db, _)| db);
            self.view.set_match_gain(key, Some(gain));
            now.push(key.clone());
        }
        self.fx_meter_buf = buf;
        let off: Vec<DeviceKey> = self
            .blind
            .shown
            .iter()
            .filter(|k| !now.contains(k))
            .cloned()
            .collect();
        for key in off {
            self.view.set_match_gain(&key, None);
        }
        self.blind.shown = now.into_iter().collect();
    }
}
