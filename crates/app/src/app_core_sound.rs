//! The Sound Analyst in the app core: **File > Sound Analyst** opens a panel that takes an audio
//! file ("recreate this sound"), measures its envelope, brightness, pitch, harmonics and noise
//! ([`atomik_dsp::sound_analysis`]) and proposes a starting patch on a built-in synth
//! ([`atomik_render::sound_analyst`] has the rules), rendered once on the note it will play so the
//! panel can show how close it is.
//!
//! [`AppCore::analyse_sound_file`] decodes, measures and fits on a worker thread
//! ([`crate::sound_worker`]); the analysis never changes the song. [`AppCore::make_sound_patch`]
//! is the one thing that does: a new instrument track with the proposed synth, its params set and a
//! one-note clip that plays the sound's note, **one undo step**, named in the status line.
//! [`AppCore::start_sound_match`] renders the patch that was made, with the params it has now, on
//! the same note and compares its three curves with the sound's again, so a patch tweaked by hand
//! can be measured; a match of a patch that was changed afterwards is shown faded.
//!
//! Opening or creating a project drops the analysis (the worker's result is ignored).

use super::*;
use crate::sound_worker::{self, Analysed, Outcome, Work};
use atomik_core::PPQ;
use atomik_dsp::harmony::SoundChord;
use atomik_dsp::sound_analysis::{
    Distances, HarmonicKind, PitchMotion, STATIC_OCTAVES, SoundProfile,
};
use atomik_model::{Clip, ClipContent, CreateOptions, CreateSource, Note};
use atomik_render::sound_analyst::Proposal;
use atomik_ui::{Curve, Distance, MatchRow, SoundAnalystData};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// What a running job is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Match,
}

/// The job that runs.
struct Running {
    job: u64,
    kind: Kind,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    /// The params of the made patch a match job renders.
    params: Option<BTreeMap<String, f32>>,
}

/// The patch that was made from the analysis.
struct Made {
    /// The device of the new track.
    device: DeviceKey,
    /// The proposal it was made from (the note and the length of the sound).
    proposal: Proposal,
}

/// A measured match.
struct Matched {
    /// The render's curves and distances.
    rendered: SoundProfile,
    distances: Distances,
    /// The params of the made patch it was measured on; `None` for the proposal itself.
    params: Option<BTreeMap<String, f32>>,
}

/// The Sound Analyst state of an open project.
#[derive(Default)]
pub(super) struct SoundState {
    running: Option<Running>,
    next_job: u64,
    analysed: Option<Analysed>,
    made: Option<Made>,
    matched: Option<Matched>,
}

impl SoundState {
    /// Stops the running job and forgets everything (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.analysed = None;
        self.made = None;
        self.matched = None;
    }
}

/// The name of a (fractional) MIDI key: `A3`, `C#4`.
pub(super) fn note_name(key: f32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let k = key.round() as i32;
    format!(
        "{}{}",
        NAMES[k.rem_euclid(12) as usize],
        k.div_euclid(12) - 1
    )
}

/// The level curve in dB of a profile.
fn level_db(p: &SoundProfile) -> Vec<f32> {
    p.level
        .iter()
        .map(|l| 20.0 * l.max(1.0e-5).log10())
        .collect()
}

/// The three curves of the target and, when there is one, of a render.
fn curves(target: &SoundProfile, made: Option<&SoundProfile>) -> (Curve, Curve, Option<Curve>) {
    let pair = |t: Vec<f32>, m: Option<Vec<f32>>| Curve {
        target: t,
        target_hop_s: target.hop_s,
        made: m.unwrap_or_default(),
        made_hop_s: made.map_or(target.hop_s, |p| p.hop_s),
    };
    (
        pair(level_db(target), made.map(level_db)),
        pair(
            target.centroid_hz.clone(),
            made.map(|p| p.centroid_hz.clone()),
        ),
        target
            .pitched()
            .then(|| pair(target.pitch_hz.clone(), made.map(|p| p.pitch_hz.clone()))),
    )
}

/// The chord of a sound as a line: its name and its notes, `Chord: Am (A3, C4, E4)`.
fn chord_line(c: &SoundChord) -> String {
    let notes: Vec<String> = c.keys.iter().map(|k| note_name(f32::from(*k))).collect();
    format!(
        "Chord: {} ({})",
        atomik_model::format_chord(c.root, c.quality, c.bass),
        notes.join(", ")
    )
}

/// The numbers of a profile, one line each. A sound that holds a chord shows it in place of its
/// pitch.
fn numbers(p: &SoundProfile, chord: Option<&SoundChord>) -> Vec<String> {
    let e = &p.envelope;
    let envelope = if e.sustain > 0.0 {
        format!(
            "Envelope: attack {:.0} ms, decay {:.0} ms to a sustain of {:.0} %, held {:.2} s, \
             release {:.0} ms",
            e.attack_s * 1000.0,
            e.decay_s * 1000.0,
            e.sustain * 100.0,
            e.hold_s,
            e.release_s * 1000.0
        )
    } else {
        format!(
            "Envelope: attack {:.0} ms, then it dies away in {:.0} ms (no sustain)",
            e.attack_s * 1000.0,
            e.decay_s * 1000.0
        )
    };
    let b = &p.brightness;
    let octaves = (b.start_hz.max(1.0) / b.end_hz.max(1.0)).log2();
    let brightness = if octaves.abs() >= STATIC_OCTAVES && b.fall_s > 0.0 {
        format!(
            "Brightness: {:.0} Hz at the peak, {:.0} Hz at the end of the note, in {:.0} ms",
            b.start_hz,
            b.end_hz,
            b.fall_s * 1000.0
        )
    } else {
        format!("Brightness: steady around {:.0} Hz", b.mean_hz)
    };
    let pitch = match p.pitch {
        _ if chord.is_some() => chord.map(chord_line).unwrap_or_default(),
        Some(PitchMotion {
            fundamental_hz,
            key,
            start_semitones,
            settle_s,
        }) if start_semitones != 0.0 => format!(
            "Pitch: {fundamental_hz:.1} Hz ({}), starting {start_semitones:+.1} semitones off and \
             settling in {:.0} ms",
            note_name(key),
            settle_s * 1000.0
        ),
        Some(m) => format!(
            "Pitch: {:.1} Hz ({}), steady",
            m.fundamental_hz,
            note_name(m.key)
        ),
        None if p.harmonics.kind == HarmonicKind::Inharmonic => {
            "Pitch: none, the partials do not line up".to_owned()
        }
        None => "Pitch: unpitched".to_owned(),
    };
    let partials: Vec<String> = p
        .harmonics
        .partials
        .iter()
        .take(4)
        .map(|q| format!("{:.2}", q.ratio))
        .collect();
    let harmonics = format!(
        "Harmonics: {}, noise {:.0} %{}",
        p.harmonics.kind.text(),
        p.noise_share * 100.0,
        if p.harmonics.kind == HarmonicKind::Inharmonic && !partials.is_empty() {
            format!(", partials at {}", partials.join(", "))
        } else {
            String::new()
        }
    );
    vec![envelope, brightness, pitch, harmonics]
}

fn distance(value: f32, unit: &'static str, score: f32) -> Distance {
    Distance { value, unit, score }
}

impl<S: EngineSink> AppCore<S> {
    /// The device of the patch that was made, while it is in the document.
    fn made_device(&self) -> Option<&RackDevice> {
        let made = self.sound.made.as_ref()?;
        self.doc().device(&made.device)
    }

    /// The patch that was made as it is now: its device type with the params it has.
    fn made_proposal(&self) -> Option<Proposal> {
        let device = self.made_device()?;
        let mut proposal = self.sound.made.as_ref()?.proposal.clone();
        proposal.params = device.params.iter().map(|(k, v)| (k.clone(), *v)).collect();
        Some(proposal)
    }

    fn start_sound_job(&mut self, kind: Kind, work: Work) -> Result<(), String> {
        if self.sound.running.is_some() {
            let why = "The Sound Analyst is already working".to_owned();
            self.reject(why.clone());
            return Err(why);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.sound.next_job;
        self.sound.next_job += 1;
        let started = sound_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&progress)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.sound.running = Some(Running {
                    job,
                    kind,
                    cancel,
                    progress,
                    params: None,
                });
                Ok(())
            }
            Err(e) => {
                let why = format!("cannot start the analysis: {e}");
                self.reject(why.clone());
                Err(why)
            }
        }
    }

    /// Starts analysing the audio file `path` (see the module docs). One job runs at a time.
    ///
    /// # Errors
    /// A job already runs, or the worker cannot start (the reason is also in the status line).
    /// A file that cannot be read or holds no sound fails later, on the worker.
    pub fn analyse_sound_file(&mut self, path: &Path) -> Result<(), String> {
        self.start_sound_job(Kind::File, Work::File(path.to_path_buf()))
    }

    /// Whether the Sound Analyst works.
    pub fn sound_running(&self) -> bool {
        self.sound.running.is_some()
    }

    /// How far the running job is, `0.0..=1.0`.
    pub fn sound_progress(&self) -> Option<f32> {
        let r = self.sound.running.as_ref()?;
        Some(f32::from_bits(r.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0))
    }

    /// Stops the running job.
    pub fn cancel_sound(&mut self) {
        if let Some(r) = &self.sound.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// What the last file analysis found.
    pub fn sound_analysis(&self) -> Option<&Analysed> {
        self.sound.analysed.as_ref()
    }

    /// The key of the device that Make Patch made, while it is in the document.
    pub fn sound_patch_device(&self) -> Option<&DeviceKey> {
        self.made_device().map(|d| &d.key)
    }

    /// A one-note clip that plays `proposal`'s note once, at least a bar long.
    fn sound_clip(&self, proposal: &Proposal) -> Clip {
        let beats = f64::from(proposal.hold_s) * self.doc().tempo_bpm / 60.0;
        #[allow(clippy::cast_possible_truncation)] // a few beats
        let (note, bar) = (((beats * PPQ as f64).round() as i64).max(PPQ / 8), 4 * PPQ);
        Clip {
            id: ClipId(0),
            start: 0,
            length: (note + bar - 1) / bar * bar,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            follow_song_key: true,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(
                proposal
                    .keys()
                    .into_iter()
                    .map(|key| Note::new(0, note, key, 1.0))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        }
    }

    /// Adds the proposed patch: a new instrument track on the proposal's synth with its params set
    /// and a clip that plays the note, in one undo step.
    ///
    /// # Errors
    /// Nothing is analysed yet, a job runs, or the model refuses (nothing is added then).
    pub fn make_sound_patch(&mut self) -> Result<(), String> {
        let refusal = if self.sound.running.is_some() {
            Some("Wait for the analysis to finish".to_owned())
        } else if self.sound.analysed.is_none() {
            Some("Analyse a sound first".to_owned())
        } else {
            None
        };
        if let Some(why) = refusal {
            self.reject(why.clone());
            return Err(why);
        }
        let Some(analysed) = self.sound.analysed.as_ref() else {
            return Err("Analyse a sound first".to_owned());
        };
        let proposal = analysed.fit.proposal.clone();
        let rendered = analysed.fit.rendered.clone();
        let distances = analysed.fit.distances;
        let name = analysed.name.clone();
        let created = self.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(proposal.type_id().to_owned()),
            key: None,
            options: CreateOptions::default(),
        }));
        if let Err(e) = created {
            let why = format!("Cannot add the patch: {e}");
            self.reject(why.clone());
            return Err(why);
        }
        // The creation selects the instrument it made.
        let device = self
            .view
            .selected()
            .filter(|k| {
                self.doc()
                    .device(k)
                    .is_some_and(|d| d.type_id == proposal.type_id())
            })
            .cloned();
        let Some(device) = device else {
            self.undo();
            let why = "Cannot add the patch: no instrument was created".to_owned();
            self.reject(why.clone());
            return Err(why);
        };
        let mut batch: Vec<Command> = proposal
            .params
            .iter()
            .map(|(param, value)| Command::SetParam {
                key: device.clone(),
                param: param.clone(),
                value: *value,
                merge: None,
            })
            .collect();
        batch.push(Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane: 0,
            clip: self.sound_clip(&proposal),
        }));
        // The params and the clip join the creation: one undo step.
        self.join_next = true;
        let applied = self.apply_command(Command::Batch(batch));
        self.join_next = false;
        if let Err(e) = applied {
            self.undo();
            let why = format!("Cannot set the patch: {e}");
            self.reject(why.clone());
            return Err(why);
        }
        let params = self.doc().device(&device).map(|d| d.params.clone());
        let synth = self
            .doc()
            .device(&device)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        self.sound.made = Some(Made { device, proposal });
        self.sound.matched = Some(Matched {
            rendered,
            distances,
            params,
        });
        self.report(format!(
            "Made a starting patch on {synth} from {name}: {:.0} % match",
            distances.overall() * 100.0
        ));
        Ok(())
    }

    /// Renders the patch that was made, as it is now, on the sound's note and compares its curves
    /// with the sound's.
    ///
    /// # Errors
    /// No patch was made (or its device is gone), or a job runs.
    pub fn start_sound_match(&mut self) -> Result<(), String> {
        let (Some(analysed), Some(proposal)) = (&self.sound.analysed, self.made_proposal()) else {
            let why = "Make a patch first: there is nothing to match".to_owned();
            self.reject(why.clone());
            return Err(why);
        };
        let work = Work::Match {
            target: Box::new(analysed.profile.clone()),
            proposal: Box::new(proposal),
        };
        let params = self.made_device().map(|d| d.params.clone());
        self.start_sound_job(Kind::Match, work)?;
        if let Some(r) = &mut self.sound.running {
            r.params = params;
        }
        Ok(())
    }

    /// What the panel shows.
    pub fn sound_data(&self) -> SoundAnalystData {
        let running = self.sound_progress();
        let Some(analysed) = &self.sound.analysed else {
            return SoundAnalystData {
                running,
                ..SoundAnalystData::default()
            };
        };
        let profile = &analysed.profile;
        let (shown, of, params) = match &self.sound.matched {
            Some(m) => (&m.rendered, m.distances, m.params.as_ref()),
            None => (&analysed.fit.rendered, analysed.fit.distances, None),
        };
        let (level, brightness, pitch) = curves(profile, Some(shown));
        let current = self.made_device().map(|d| &d.params);
        let stale = params.is_some_and(|p| current.is_some_and(|c| p != c));
        let (e, b, p) = of.scores();
        let label = if params.is_some() {
            format!(
                "Patch on {}",
                self.made_device().map_or("the track", |d| d.name.as_str())
            )
        } else {
            "Proposed patch".to_owned()
        };
        SoundAnalystData {
            file: Some(format!("{}, {:.1} s", analysed.name, profile.duration_s)),
            running,
            numbers: numbers(profile, analysed.chord.as_ref()),
            plan: analysed.fit.proposal.reasons.clone(),
            level,
            brightness,
            pitch,
            matched: Some(MatchRow {
                of: label,
                level: distance(of.envelope_db, "dB", e),
                brightness: distance(of.brightness_octaves, "oct", b),
                pitch: of.pitch_semitones.zip(p).map(|(d, s)| distance(d, "st", s)),
                overall: of.overall(),
            }),
            can_make: running.is_none(),
            can_match: running.is_none() && self.made_device().is_some(),
            stale,
        }
    }

    /// A job finished (or failed, or was stopped).
    pub(super) fn apply_sound(&mut self, job: u64, result: Result<Option<Outcome>, String>) {
        let Some(running) = self.sound.running.take_if(|r| r.job == job) else {
            return;
        };
        match result {
            Err(e) => self.reject(format!("Sound analysis failed: {e}")),
            Ok(None) => self.reject("Sound analysis cancelled".to_owned()),
            Ok(Some(Outcome::Analysed(analysed))) if running.kind == Kind::File => {
                let a = *analysed;
                let message = format!(
                    "{}: {}; proposed {} ({:.0} % match)",
                    a.name,
                    match (&a.chord, a.profile.pitch) {
                        (Some(c), _) => format!("a chord, {}", chord_line(c)),
                        (None, Some(p)) => format!("pitched at {:.0} Hz", p.fundamental_hz),
                        (None, None) => "unpitched".to_owned(),
                    },
                    a.fit
                        .proposal
                        .reasons
                        .first()
                        .map_or("a patch", String::as_str),
                    a.fit.distances.overall() * 100.0
                );
                self.sound.made = None;
                self.sound.matched = None;
                self.sound.analysed = Some(a);
                self.reject(message);
            }
            Ok(Some(Outcome::Matched {
                rendered,
                distances,
            })) if running.kind == Kind::Match => {
                let params = running.params;
                self.reject(format!(
                    "Patch matched: {:.0} % (level {:.1} dB, brightness {:.2} octaves)",
                    distances.overall() * 100.0,
                    distances.envelope_db,
                    distances.brightness_octaves
                ));
                self.sound.matched = Some(Matched {
                    rendered: *rendered,
                    distances,
                    params,
                });
            }
            Ok(Some(_)) => {}
        }
    }
}
