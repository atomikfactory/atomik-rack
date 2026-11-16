//! The Chord Wheel note player in the app core: Generate Progression, editing a slot, Send
//! Progression to Chord Lane, Send to Clip and the chord audition.
//!
//! The device stores everything in params (see `atomik_devices::chord_wheel`), so every edit here
//! is a `SetParam` or a `Batch` of them and undo, copy and paste of settings and automation work
//! as for any device. Slots are numbered from 0 in this API (`slot.1.*` is slot 0).
//!
//! * **Generate Progression** ([`AppCore::wheel_generate`]) advances `gen.seed` (past 9999 it
//!   starts again at 0), runs [`atomik_core::progression::generate`] with the key, mood, spice and
//!   slot count of the device and the chords of the locked slots, and writes root, quality, bass
//!   and length of the active slots that are not locked. One undo step.
//! * **Set Wheel Chord** ([`AppCore::wheel_set_chord`]) writes root, quality and bass of one slot
//!   as a ganged `SetParam` batch with a merge id of that slot, so consecutive sets of one slot are
//!   one undo step (as a value drag is); any other command ends the group.
//! * **Send Progression to Chord Lane** ([`AppCore::wheel_to_chord_lane`]) writes the active slots
//!   as chord-lane chords (a slot is `beats` quarter notes long, the unit the device plays) from
//!   the left locator (the loop start): once, or, with the loop on, repeated to the loop end
//!   (the last chord is cut there). The chords of the lane inside the written span are replaced.
//!   With `key.source` = song the chords move by [`song_key_shift`] to the song key at each chord;
//!   a song without a key gets the device key. The chords and the key are one
//!   `SetHarmony`: one undo step.
//! * **Send to Clip** ([`AppCore::wheel_to_clip`]) is Send Players to Track for the track of the
//!   player over the same span (see [`SendWindow`](super::player_stack_ops::SendWindow)): the notes
//!   the stack plays for one pass of the progression, repeated over the span, on a new note lane
//!   named "Chord Wheel".
//! * **Audition** ([`AppCore::wheel_audition`]) voices one chord at the device's register, spread
//!   and extension and plays it through what the player's `notes_out` is cabled to, with the chord
//!   audition of the chord lane. It does nothing while the song plays.

use super::*;
use atomik_core::PPQ;
use atomik_core::progression::{
    BEATS, Chord, Extend, Key as WheelKey, KeyMode, MAX_SLOTS, Mood, Spread, VoiceOpts,
    beats_index, generate, song_key_shift, voice,
};
use atomik_model::theory::{ChordKind, ScaleKind};
use atomik_model::{ChordEvent, Harmony, Key as SongKey, player_stack};

/// The device type of the Chord Wheel.
pub const CHORD_WHEEL: &str = "atomik.chord-wheel";

/// The merge id base of the slot edits (slot `k` is `base + k`).
const WHEEL_MERGE_BASE: u64 = 21 << 40;

/// The lane name of what Send to Clip writes.
const CLIP_LANE_NAME: &str = "Chord Wheel";

/// The params of slot `k` (0-based): root, quality, bass, beats, lock.
fn slot_param(k: usize, field: &str) -> String {
    format!("slot.{}.{field}", k + 1)
}

/// The params that make up one chord of a slot, in the order they are written.
fn chord_params(k: usize) -> [String; 3] {
    [
        slot_param(k, "root"),
        slot_param(k, "quality"),
        slot_param(k, "bass"),
    ]
}

/// The quality param value of `kind`.
fn quality_value(kind: ChordKind) -> f32 {
    ChordKind::ALL.iter().position(|k| *k == kind).unwrap_or(0) as f32
}

/// The bass param value of `chord`: 0 for the root, else the slash bass pitch class plus one.
fn bass_value(chord: Chord) -> f32 {
    chord.bass.map_or(0.0, |b| f32::from(b % 12) + 1.0)
}

/// A slot length in ticks: `beats` quarter notes (what the device plays).
fn beats_ticks(beats: u8) -> i64 {
    i64::from(beats) * PPQ
}

/// The key of the song for a device key.
fn song_key_of(key: WheelKey) -> SongKey {
    SongKey::new(
        key.root,
        match key.mode {
            KeyMode::Major => ScaleKind::Major,
            KeyMode::Minor => ScaleKind::NaturalMinor,
        },
    )
}

impl<S: EngineSink> AppCore<S> {
    /// Whether `device` is a Chord Wheel of the document.
    pub(crate) fn is_chord_wheel(&self, device: &DeviceKey) -> bool {
        self.doc()
            .resolve(device)
            .is_some_and(|d| d.type_id == CHORD_WHEEL)
    }

    /// The Chord Wheel the commands act on: the device selected in the rack when it is one, else
    /// the first Chord Wheel in the player stacks of the selected tracks.
    pub fn chord_wheel_selected(&self) -> Option<DeviceKey> {
        if let Some(d) = self.view.selected()
            && self.is_chord_wheel(d)
        {
            return Some(d.clone());
        }
        let doc = self.doc();
        self.arrangement
            .command_tracks()
            .into_iter()
            .filter_map(|d| {
                let t = doc.song.track(&d)?;
                player_stack(doc, &self.registry, t)
                    .into_iter()
                    .find(|k| self.is_chord_wheel(k))
            })
            .next()
    }

    /// The track whose player stack holds `wheel`.
    fn wheel_track(&self, wheel: &DeviceKey) -> Option<DeviceKey> {
        let doc = self.doc();
        doc.song
            .tracks
            .iter()
            .find(|t| player_stack(doc, &self.registry, t).contains(wheel))
            .map(|t| t.device.clone())
    }

    /// The value of a param of `wheel` (the stored value or the default).
    fn wheel_value(&self, wheel: &DeviceKey, param: &str) -> f32 {
        self.doc()
            .param_value(wheel, param, &self.registry)
            .unwrap_or(0.0)
    }

    /// The device key of `wheel`.
    fn wheel_key(&self, wheel: &DeviceKey) -> WheelKey {
        let mode = if self.wheel_value(wheel, "key.mode") >= 0.5 {
            KeyMode::Minor
        } else {
            KeyMode::Major
        };
        WheelKey::new(self.wheel_value(wheel, "key.root").round() as u8, mode)
    }

    /// The number of active slots of `wheel`, `1..=8`.
    fn wheel_slots(&self, wheel: &DeviceKey) -> usize {
        (self.wheel_value(wheel, "slots").round() as usize).clamp(1, MAX_SLOTS)
    }

    /// The chord of slot `k` of `wheel`, as written (in the device key).
    fn wheel_chord(&self, wheel: &DeviceKey, k: usize) -> Chord {
        let quality = (self.wheel_value(wheel, &slot_param(k, "quality")).round() as usize)
            .min(ChordKind::ALL.len() - 1);
        let chord = Chord::new(
            self.wheel_value(wheel, &slot_param(k, "root")).round() as u8,
            ChordKind::ALL[quality],
        );
        match self.wheel_value(wheel, &slot_param(k, "bass")).round() as u8 {
            0 => chord,
            b => chord.over(b - 1),
        }
    }

    /// The length of slot `k` of `wheel` in beats.
    fn wheel_beats(&self, wheel: &DeviceKey, k: usize) -> u8 {
        let i = self.wheel_value(wheel, &slot_param(k, "beats")).round() as usize;
        BEATS[i.min(BEATS.len() - 1)]
    }

    /// Whether slot `k` of `wheel` is locked.
    fn wheel_locked(&self, wheel: &DeviceKey, k: usize) -> bool {
        self.wheel_value(wheel, &slot_param(k, "lock")) >= 0.5
    }

    /// Refuses (with a message) a device that is not a Chord Wheel.
    fn wheel_checked(&mut self, wheel: &DeviceKey) -> bool {
        let ok = self.is_chord_wheel(wheel);
        if !ok {
            self.reject(format!("{wheel} is not a Chord Wheel"));
        }
        ok
    }

    fn wheel_set(wheel: &DeviceKey, param: String, value: f32, merge: Option<u64>) -> Command {
        Command::SetParam {
            key: wheel.clone(),
            param,
            value,
            merge,
        }
    }

    // ------------------------------------------------------------------ generate

    /// Generate Progression on the selected Chord Wheel (see [`Self::wheel_generate`]); a
    /// message, and nothing else, when none is selected.
    pub fn wheel_generate_selected(&mut self) -> bool {
        match self.chord_wheel_selected() {
            Some(wheel) => self.wheel_generate(&wheel),
            None => {
                self.reject("Generate Progression needs a selected Chord Wheel".into());
                false
            }
        }
    }

    /// Generates a progression for `wheel` from its key, mood and spice and the next seed, keeping
    /// the locked slots; see the module docs. One undo step. False, with a message, when `wheel`
    /// is not a Chord Wheel or the edit was refused.
    pub fn wheel_generate(&mut self, wheel: &DeviceKey) -> bool {
        if !self.wheel_checked(wheel) {
            return false;
        }
        let key = self.wheel_key(wheel);
        let mood = Mood::from_index(self.wheel_value(wheel, "gen.mood").round() as usize)
            .unwrap_or(Mood::Pop);
        let spice = self.wheel_value(wheel, "gen.spice").clamp(0.0, 1.0);
        let held = self.wheel_value(wheel, "gen.seed").round() as u32;
        let seed = if held >= 9999 { 0 } else { held + 1 };
        let slots = self.wheel_slots(wheel);
        let mut locks: [Option<Chord>; MAX_SLOTS] = [None; MAX_SLOTS];
        for (k, lock) in locks.iter_mut().enumerate() {
            if self.wheel_locked(wheel, k) {
                *lock = Some(self.wheel_chord(wheel, k));
            }
        }
        let made = generate(key, mood, spice, seed, slots, &locks);
        let mut batch = vec![Self::wheel_set(wheel, "gen.seed".into(), seed as f32, None)];
        for k in (0..slots).filter(|k| locks[*k].is_none()) {
            let chord = made.chords[k];
            let [root, quality, bass] = chord_params(k);
            batch.extend([
                Self::wheel_set(wheel, root, f32::from(chord.root), None),
                Self::wheel_set(wheel, quality, quality_value(chord.kind), None),
                Self::wheel_set(wheel, bass, bass_value(chord), None),
                Self::wheel_set(
                    wheel,
                    slot_param(k, "beats"),
                    beats_index(made.beats[k]) as f32,
                    None,
                ),
            ]);
        }
        self.document.end_merge();
        if self.apply_command(Command::Batch(batch)).is_err() {
            self.reject("Generate Progression was refused".into());
            return false;
        }
        true
    }

    // ------------------------------------------------------------------ edit

    /// Sets slot `slot` (`0..8`) of `wheel` to `chord` (root, quality and slash bass), one undo
    /// step; consecutive sets of the same slot are one step. False, with a message, for a device
    /// that is no Chord Wheel or a slot past the eighth.
    pub fn wheel_set_chord(&mut self, wheel: &DeviceKey, slot: usize, chord: Chord) -> bool {
        if !self.wheel_checked(wheel) {
            return false;
        }
        if slot >= MAX_SLOTS {
            self.reject(format!("the Chord Wheel has no slot {}", slot + 1));
            return false;
        }
        let merge = Some(WHEEL_MERGE_BASE + slot as u64);
        let [root, quality, bass] = chord_params(slot);
        let batch = vec![
            Self::wheel_set(wheel, root, f32::from(chord.root % 12), merge),
            Self::wheel_set(wheel, quality, quality_value(chord.kind), merge),
            Self::wheel_set(wheel, bass, bass_value(chord), merge),
        ];
        self.apply_command(Command::Batch(batch)).is_ok()
    }

    /// Locks slot `slot` (`0..8`) of `wheel` against Generate Progression, or unlocks it. One
    /// undo step.
    pub fn wheel_toggle_lock(&mut self, wheel: &DeviceKey, slot: usize) -> bool {
        if !self.wheel_checked(wheel) {
            return false;
        }
        if slot >= MAX_SLOTS {
            self.reject(format!("the Chord Wheel has no slot {}", slot + 1));
            return false;
        }
        let on = !self.wheel_locked(wheel, slot);
        let value = if on { 1.0 } else { 0.0 };
        self.document.end_merge();
        self.apply_command(Self::wheel_set(
            wheel,
            slot_param(slot, "lock"),
            value,
            None,
        ))
        .is_ok()
    }

    /// Sets the number of active slots of `wheel` (clamped to `1..=8`). One undo step.
    pub fn wheel_set_slots(&mut self, wheel: &DeviceKey, slots: usize) -> bool {
        if !self.wheel_checked(wheel) {
            return false;
        }
        let slots = slots.clamp(1, MAX_SLOTS);
        self.document.end_merge();
        self.apply_command(Self::wheel_set(wheel, "slots".into(), slots as f32, None))
            .is_ok()
    }

    // ------------------------------------------------------------------ send

    /// The span the progression of `wheel` is sent to, `(from, to, repeat)`: from the left
    /// locator, to the right one repeating the progression when the loop is on, else once for
    /// `cycle` ticks.
    fn wheel_span(&self, cycle: i64) -> (i64, i64, bool) {
        let song = &self.doc().song;
        let (from, to) = song.loop_range;
        if song.loop_enabled && to > from {
            (from, to, true)
        } else {
            (from, from.saturating_add(cycle), false)
        }
    }

    /// The ticks of one pass of the active slots of `wheel`.
    fn wheel_cycle(&self, wheel: &DeviceKey) -> i64 {
        (0..self.wheel_slots(wheel))
            .map(|k| beats_ticks(self.wheel_beats(wheel, k)))
            .sum::<i64>()
            .max(1)
    }

    /// Send Progression to Chord Lane for the selected Chord Wheel; a message when none is.
    pub fn wheel_to_chord_lane_selected(&mut self) -> bool {
        match self.chord_wheel_selected() {
            Some(wheel) => self.wheel_to_chord_lane(&wheel),
            None => {
                self.reject("Send Progression to Chord Lane needs a selected Chord Wheel".into());
                false
            }
        }
    }

    /// Writes the progression of `wheel` into the chord lane over its span (see the module
    /// docs). One undo step. False, with a message, when nothing was written.
    pub fn wheel_to_chord_lane(&mut self, wheel: &DeviceKey) -> bool {
        if !self.wheel_checked(wheel) {
            return false;
        }
        let slots = self.wheel_slots(wheel);
        let chords: Vec<(Chord, i64)> = (0..slots)
            .map(|k| {
                (
                    self.wheel_chord(wheel, k),
                    beats_ticks(self.wheel_beats(wheel, k)),
                )
            })
            .collect();
        let cycle = chords.iter().map(|(_, l)| *l).sum::<i64>().max(1);
        let (from, to, repeat) = self.wheel_span(cycle);
        let device_key = self.wheel_key(wheel);
        let follow_song = self.wheel_value(wheel, "key.source") >= 0.5;
        let harmony = self.harmony().clone();
        let mut found: Vec<ChordEvent> = Vec::new();
        let mut tick = from;
        'fill: loop {
            for (chord, length) in &chords {
                if tick >= to {
                    break 'fill;
                }
                let shift = harmony
                    .key_at(tick)
                    .filter(|_| follow_song)
                    .and_then(|k| song_key_shift(device_key, k.as_scale()))
                    .unwrap_or(0);
                let c = chord.transposed(shift);
                found.push(ChordEvent {
                    tick,
                    length: (*length).min(to - tick),
                    root: c.root,
                    quality: c.kind,
                    bass: c.bass,
                });
                tick += length;
            }
            if !repeat {
                break;
            }
        }
        let mut next: Harmony = match harmony.with_chords_in_range((from, to), &found, false) {
            Ok(h) => h,
            Err(e) => {
                self.reject(format!(
                    "Cannot send the progression to the chord lane: {e}"
                ));
                return false;
            }
        };
        let mut said_key = None;
        if next.key.is_none() {
            let key = song_key_of(device_key);
            if let Ok(h) = next.with_key(Some(key)) {
                next = h;
                said_key = Some(key);
            }
        }
        if next == harmony {
            self.report("The chord lane already holds this progression");
            return true;
        }
        if self.set_harmony(next).is_err() {
            return false;
        }
        let n = found.len();
        let mut said = format!(
            "{n} chord{} put in the chord lane",
            if n == 1 { "" } else { "s" }
        );
        if let Some(k) = said_key {
            said.push_str(&format!(", song key {}", k.name()));
        }
        self.report(format!("{said}; Undo takes them out"));
        true
    }

    /// Send to Clip for the selected Chord Wheel; a message when none is.
    pub fn wheel_to_clip_selected(&mut self) -> bool {
        match self.chord_wheel_selected() {
            Some(wheel) => self.wheel_to_clip(&wheel),
            None => {
                self.reject("Send Progression to Clip needs a selected Chord Wheel".into());
                false
            }
        }
    }

    /// Sends what the track of `wheel` plays from the progression to a clip on a new note lane
    /// (see the module docs). One undo step. False, with a message, when nothing was written.
    pub fn wheel_to_clip(&mut self, wheel: &DeviceKey) -> bool {
        if !self.wheel_checked(wheel) {
            return false;
        }
        let Some(track) = self.wheel_track(wheel) else {
            self.reject(
                "Send to Clip needs the Chord Wheel on a track, above its instrument".into(),
            );
            return false;
        };
        let cycle = self.wheel_cycle(wheel);
        let (from, to, _) = self.wheel_span(cycle);
        let window = super::player_stack_ops::SendWindow { cycle, from, to };
        self.send_stack_window(&track, CLIP_LANE_NAME, Some(window))
    }

    // ------------------------------------------------------------------ audition

    /// Plays `chord` voiced like the device voices it (its register, spread and extension, root
    /// position, the slash bass below) through what `wheel` is cabled to, for a moment. Stopped
    /// transport only: while the song plays, and for a device that is no Chord Wheel, it does
    /// nothing. The chord is taken as it is, in the device key (the song key shift of a wheel that
    /// follows the song is not applied).
    pub fn wheel_audition(&mut self, wheel: &DeviceKey, chord: Chord) {
        if self.playing || !self.is_chord_wheel(wheel) {
            return;
        }
        let opts = VoiceOpts {
            key: self.wheel_key(wheel),
            lead: false,
            register: self.wheel_value(wheel, "voice.register").round() as u8,
            spread: Spread::from_index(self.wheel_value(wheel, "voice.spread").round() as usize),
            extend: Extend::from_index(self.wheel_value(wheel, "voice.extend").round() as usize),
        };
        let voiced = voice(chord, None, opts);
        let mut keys: Vec<u8> = voiced.as_slice().to_vec();
        if let (Some(bass), Some(low)) = (chord.bass, voiced.lowest()) {
            // The slash bass: its pitch class just below the lowest note of the voicing.
            let low = i32::from(low);
            let mut below = low - (low - i32::from(bass)).rem_euclid(12);
            if below == low {
                below -= 12;
            }
            if let Ok(k) = u8::try_from(below) {
                keys.insert(0, k);
            }
        }
        let velocity = self.wheel_value(wheel, "velocity").clamp(0.05, 1.0);
        let track = self.wheel_track(wheel);
        self.audition_keys_via(track.as_ref(), wheel, &keys, velocity);
    }
}

#[cfg(test)]
#[path = "app_core_chord_wheel_tests.rs"]
mod tests;
