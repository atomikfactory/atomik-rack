//! The kick and bass check: what a kick track and a bass track do to each other's sub range,
//! and the fixes that can be tried on them.
//!
//! [`analyse`] runs on a worker, never on the audio thread or the UI thread, and is
//! deterministic: it renders the two channel strips of the song (the first
//! [`CHECK_SECONDS`] of it) with the offline renderer of a stem export, in one pass, mixes each to
//! mono and hands them to [`atomik_dsp::kick_bass::check`]. The two stems come from the same
//! pass, so the delay compensation of the project is applied to both as in a bounce; their sum
//! is what the two channels add to the master, before anything the master does. A plugin of the
//! project is rendered as its stand-in (silent for an instrument, the dry input for an effect),
//! so a kick or a bass that is a plugin makes no sound here.
//!
//! When the check finds a problem (a hit that cancels, or two fundamentals that beat) it tries
//! the fixes that apply to these two tracks, **each one by building the fixed song on a scratch
//! copy, rendering it again and checking it again**, so what the panel shows for a fix is the
//! strip it really gives:
//!
//! * [`Fix::StartPhase`]: the bass is a poly synth, mono bass or wavetable synth. The
//!   oscillators are set to restart at a fixed angle (0, 90, 180 or 270 degrees: the best of
//!   the four is offered).
//! * [`Fix::Polarity`]: a Stereo Tool on the bass channel inverts both channels.
//! * [`Fix::Rotate`]: a Stereo Tool on the bass channel turns the phase by the angle that gives
//!   the best sum, at the bass fundamental.
//! * [`Fix::Delay`]: the bass channel's latency adjust delays it by the number of frames that
//!   gives the best sum (at most one period of its fundamental).
//! * [`Fix::KickPitch`]: the kick is a drum machine; the fine tune of the pad it plays is nudged
//!   by -100, -50, 50 or 100 cents, the best of the four.
//!
//! A fix is offered only when its strip scores at least [`MIN_GAIN`] better than the strip it
//! came from ([`atomik_dsp::kick_bass::Check::score`]). Applying a fix runs the same steps on
//! the song as one undo step ([`apply_fix`]). The check itself never changes the song.

use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_dsp::RotationFix;
use atomik_dsp::kick_bass::{self, Check};
use atomik_model::ConsoleChannel;
use atomik_model::{
    Command, ConsoleCommand, DeviceKey, Document, InsertEffect, RackDoc, StemSource, insert_chain,
};
use atomik_render::export::ExportSpan;
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use atomik_render::stems::{Stem, StemRequest, StemStrip, render_stems};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// How much of the song the check renders, in seconds from the start.
pub const CHECK_SECONDS: f64 = 20.0;
/// A fix is offered when its strip scores at least this much (dB) better than the strip before.
pub const MIN_GAIN: f32 = 0.5;
/// The Stereo Tool that polarity and rotation use.
pub const STEREO_TOOL: &str = "atomik.stereo-tool";
/// The instruments with an oscillator start phase.
const PHASE_TYPES: [&str; 3] = [
    "atomik.poly-synth",
    "atomik.mono-bass",
    "atomik.wavetable-synth",
];
/// The drum machine and the key of its first pad.
const DRUM_TYPE: &str = "atomik.drum-machine";
const DRUM_PADS: u8 = 16;
/// The fine-tune nudges tried on a kick pad, cents.
const NUDGES: [f32; 4] = [-100.0, -50.0, 50.0, 100.0];
/// The start angles tried on a bass oscillator, degrees.
const START_ANGLES: [f32; 4] = [0.0, 90.0, 180.0, 270.0];
/// Where the bass fundamental is taken when none was found, Hz.
const FALLBACK_HZ: f32 = 50.0;

/// The two tracks of a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    /// The device of the kick track.
    pub kick: DeviceKey,
    /// The device of the bass track.
    pub bass: DeviceKey,
}

/// One fix, with the numbers it was found with.
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    /// Restart the bass oscillators at `degrees` on every note.
    StartPhase {
        /// The start angle.
        degrees: f32,
    },
    /// Invert the polarity of the bass.
    Polarity,
    /// Turn the phase of the bass by `degrees` at `hz`.
    Rotate {
        /// The angle.
        degrees: f32,
        /// The frequency the angle is reached at.
        hz: f32,
    },
    /// Delay the bass by `frames`.
    Delay {
        /// The delay in frames.
        frames: u32,
    },
    /// Nudge the fine tune of pad `pad` (1 to 16) of the kick drum machine by `cents`.
    KickPitch {
        /// The pad number.
        pad: u8,
        /// The nudge, cents.
        cents: f32,
    },
}

impl Fix {
    /// What the fix does, in a line.
    pub fn label(&self, sample_rate: u32) -> String {
        match self {
            Self::StartPhase { degrees } => {
                format!("Start the bass oscillator at {degrees:.0} degrees on every note")
            }
            Self::Polarity => "Invert the polarity of the bass".to_owned(),
            Self::Rotate { degrees, hz } => {
                format!("Rotate the phase of the bass by {degrees:.0} degrees at {hz:.0} Hz")
            }
            Self::Delay { frames } => {
                let ms = f64::from(*frames) * 1000.0 / f64::from(sample_rate.max(1));
                format!("Delay the bass by {ms:.2} ms ({frames} samples)")
            }
            Self::KickPitch { pad, cents } => {
                format!("Tune the kick (pad {pad}) {cents:+.0} cents")
            }
        }
    }
}

/// A fix with the strip it gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Trial {
    /// The fix.
    pub fix: Fix,
    /// The check of the song with the fix applied (a re-render).
    pub check: Check,
}

/// The result of an analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The tracks it is about.
    pub setup: Setup,
    /// The check of the song as it is.
    pub base: Check,
    /// The fixes found, the best first.
    pub trials: Vec<Trial>,
    /// What was said about the fixes that could not be tried (a bass that is no synth, a kick
    /// that is no drum machine).
    pub notes: Vec<String>,
    /// The phase rotation that evens out the positive and negative peaks of the bass, when one
    /// gains headroom (see [`Headroom`]).
    pub headroom: Option<Headroom>,
}

/// The "fix asymmetry" helper: the bass swings further one way than the other, and a phase
/// rotation can even the two out, which lowers its peak without changing how it sounds alone.
#[derive(Debug, Clone, PartialEq)]
pub struct Headroom {
    /// What the search found: the angle, the peaks before and after and the headroom gained.
    pub found: RotationFix,
    /// The rotation with the strip the song gives with it. A rotation that is inaudible alone
    /// can change how the bass adds to the kick, which is why it is rendered and checked too.
    pub trial: Trial,
}

/// The two channel strips as mono signals.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    /// The kick, mono.
    pub kick: Vec<f32>,
    /// The bass, mono.
    pub bass: Vec<f32>,
    /// Their rate.
    pub sample_rate: u32,
}

/// The channel strip that plays `track`, as a stem source.
fn channel_source(doc: &RackDoc, registry: &Registry, track: &DeviceKey) -> Option<StemSource> {
    let channel = doc
        .console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(track))
        .map(|ConsoleChannel { channel, .. }| channel)?;
    atomik_model::stem_sources(doc, registry)
        .into_iter()
        .find(|s| &s.key == channel)
}

/// The channel device of `track`.
pub fn channel_of(doc: &RackDoc, track: &DeviceKey) -> Option<DeviceKey> {
    doc.console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(track))
        .map(|c| c.channel.clone())
}

/// The registry a check renders with: the built-in devices, and a stand-in for every plugin of
/// `doc`.
///
/// # Errors
/// The stand-ins cannot be built.
pub fn registry_for(doc: &RackDoc) -> Result<Registry, String> {
    let mut registry = atomik_render::default_registry();
    atomik_model::register_stand_ins(doc, &mut registry).map_err(|e| e.to_string())?;
    Ok(registry)
}

/// Renders the strips of the two tracks of `setup` (see the module docs), with `registry` from
/// [`registry_for`].
///
/// # Errors
/// A track has no channel, the song is empty, or the render fails; the text says which.
pub fn render_pair(
    doc: &RackDoc,
    registry: &Registry,
    bundle_root: Option<&Path>,
    setup: &Setup,
    cancel: Option<&AtomicBool>,
) -> Result<Option<Pair>, String> {
    let name = |key: &DeviceKey| {
        doc.device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    };
    let source = |track: &DeviceKey| {
        channel_source(doc, registry, track)
            .ok_or_else(|| format!("'{}' has no mixer channel", name(track)))
    };
    let sources = vec![source(&setup.kick)?, source(&setup.bass)?];
    let end = doc.song.end_tick();
    if end <= 0 {
        return Err("the song is empty".to_owned());
    }
    let limit = doc
        .song
        .frame_clock(doc.sample_rate)
        .tick_at((CHECK_SECONDS * f64::from(doc.sample_rate)) as u64)
        .max(PPQ);
    let request = StemRequest {
        span: ExportSpan::Range(0, end.min(limit)),
        tail_seconds: 0.0,
        strip: StemStrip::Full,
        sources,
    };
    let mut got: Vec<Option<Vec<f32>>> = vec![None, None];
    let mut rate = doc.sample_rate;
    let mut take = |stem: Stem| -> Result<(), atomik_render::RenderError> {
        rate = stem.audio.sample_rate;
        if let [l, r, ..] = stem.audio.channels.as_slice() {
            got[stem.index] = Some(kick_bass::mono(l, r));
        } else if let Some(c) = stem.audio.channels.first() {
            got[stem.index] = Some(c.clone());
        }
        Ok(())
    };
    let mut bounce = PluginBounce {
        host: None,
        mode: atomik_device_sdk::RunMode::Offline,
        missing: MissingPlugins::StandIn,
    };
    let rendered = render_stems(
        doc,
        registry,
        (&ProjectOptions::default(), bundle_root),
        &mut bounce,
        &request,
        (
            RenderControl {
                cancel,
                progress: None,
            },
            &mut take,
        ),
    )
    .map_err(|e| e.to_string())?;
    if rendered.is_none() {
        return Ok(None);
    }
    match (got[0].take(), got[1].take()) {
        (Some(kick), Some(bass)) => Ok(Some(Pair {
            kick,
            bass,
            sample_rate: rate,
        })),
        _ => Err("the render gave no audio for the two tracks".to_owned()),
    }
}

/// The scratch copy a trial is built on: a document with its history, and the registry.
struct Scratch<'a> {
    document: Document,
    registry: &'a Registry,
}

/// Where the steps of a fix go: a scratch copy for a trial, the open song for the real thing.
pub trait Steps {
    /// The document as it is now.
    fn doc(&self) -> &RackDoc;
    /// The registry the document is edited with.
    fn registry(&self) -> &Registry;
    /// Runs one command; `join` folds it into the undo step before it.
    ///
    /// # Errors
    /// The model refuses the command: its text.
    fn run(&mut self, command: Command, join: bool) -> Result<(), String>;
}

impl Steps for Scratch<'_> {
    fn doc(&self) -> &RackDoc {
        self.document.doc()
    }

    fn registry(&self) -> &Registry {
        self.registry
    }

    fn run(&mut self, command: Command, join: bool) -> Result<(), String> {
        let applied = if join {
            self.document.apply_joined(command, self.registry)
        } else {
            self.document.apply(command, self.registry)
        };
        applied.map(|_| ()).map_err(|e| e.to_string())
    }
}

/// The Stereo Tool of the insert chain of `channel`: the first one already there, else a new one
/// at the end of the chain. `first` says whether a step has run yet (the first step starts the
/// undo step, the rest join it).
fn stereo_tool_of(
    steps: &mut dyn Steps,
    channel: &DeviceKey,
    first: &mut bool,
) -> Result<DeviceKey, String> {
    let chain = insert_chain(steps.doc(), steps.registry(), channel);
    let existing = chain.into_iter().find(|key| {
        steps
            .doc()
            .device(key)
            .is_some_and(|d| d.type_id == STEREO_TOOL)
    });
    if let Some(key) = existing {
        return Ok(key);
    }
    let slot = insert_chain(steps.doc(), steps.registry(), channel).len();
    let before: Vec<DeviceKey> = steps.doc().devices.iter().map(|d| d.key.clone()).collect();
    steps.run(
        Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot,
            effect: Some(InsertEffect::New {
                type_id: STEREO_TOOL.to_owned(),
            }),
        }),
        !std::mem::replace(first, false),
    )?;
    steps
        .doc()
        .devices
        .iter()
        .find(|d| d.type_id == STEREO_TOOL && !before.contains(&d.key))
        .map(|d| d.key.clone())
        .ok_or_else(|| "the Stereo Tool was not added".to_owned())
}

/// Applies `fix` to the song behind `steps` (see the module docs) and returns the text of the
/// status line. The first command starts an undo step and every other joins it, so the whole fix
/// is one step.
///
/// # Errors
/// The fix does not fit the song (a device the fix needs is gone) or the model refuses a step;
/// nothing is left half done when the first step fails.
pub fn apply_fix(
    steps: &mut dyn Steps,
    setup: &Setup,
    fix: &Fix,
    sample_rate: u32,
) -> Result<String, String> {
    let mut first = true;
    let set =
        |steps: &mut dyn Steps, key: &DeviceKey, param: &str, value: f32, first: &mut bool| {
            steps.run(
                Command::SetParam {
                    key: key.clone(),
                    param: param.to_owned(),
                    value,
                    merge: None,
                },
                !std::mem::replace(first, false),
            )
        };
    let bass_channel = |steps: &dyn Steps| {
        channel_of(steps.doc(), &setup.bass)
            .ok_or_else(|| "the bass track has no mixer channel".to_owned())
    };
    match fix {
        Fix::StartPhase { degrees } => {
            let device = setup.bass.clone();
            let type_ok = steps
                .doc()
                .resolve(&device)
                .is_some_and(|d| PHASE_TYPES.contains(&d.type_id.as_str()));
            if !type_ok {
                return Err("the bass is not a synth with a start phase".to_owned());
            }
            if steps
                .doc()
                .param_value(&device, "osc.phase", steps.registry())
                .is_some_and(|v| v >= 0.5)
            {
                set(steps, &device, "osc.phase", 0.0, &mut first)?;
            }
            set(steps, &device, "osc.phase_start", *degrees, &mut first)?;
        }
        Fix::Polarity => {
            let channel = bass_channel(steps)?;
            let tool = stereo_tool_of(steps, &channel, &mut first)?;
            for param in ["invert_l", "invert_r"] {
                let now = steps
                    .doc()
                    .param_value(&tool, param, steps.registry())
                    .unwrap_or(0.0);
                set(
                    steps,
                    &tool,
                    param,
                    if now >= 0.5 { 0.0 } else { 1.0 },
                    &mut first,
                )?;
            }
        }
        Fix::Rotate { degrees, hz } => {
            let channel = bass_channel(steps)?;
            let tool = stereo_tool_of(steps, &channel, &mut first)?;
            set(steps, &tool, "rotate_hz", *hz, &mut first)?;
            set(steps, &tool, "rotate", *degrees, &mut first)?;
        }
        Fix::Delay { frames } => {
            let channel = bass_channel(steps)?;
            let now = steps
                .doc()
                .param_value(&channel, "latency.adjust", steps.registry())
                .unwrap_or(0.0);
            set(
                steps,
                &channel,
                "latency.adjust",
                now + *frames as f32,
                &mut first,
            )?;
        }
        Fix::KickPitch { pad, cents } => {
            let device = setup.kick.clone();
            let param = format!("pad{pad}.fine");
            let now = steps
                .doc()
                .param_value(&device, &param, steps.registry())
                .ok_or_else(|| "the kick is not a drum machine".to_owned())?;
            set(
                steps,
                &device,
                &param,
                (now + *cents).clamp(-100.0, 100.0),
                &mut first,
            )?;
        }
    }
    Ok(format!("Kick and Bass: {}", fix.label(sample_rate)))
}

/// The pad of the kick drum machine that plays most of the kick track's notes (1 to 16).
fn kick_pad(doc: &RackDoc, kick: &DeviceKey) -> Option<u8> {
    let track = doc.song.track(kick)?;
    let notes = atomik_model::song::flatten_track_notes(&doc.song, track);
    let mut counts = [0u32; DRUM_PADS as usize];
    for (_, _, key, _) in notes {
        let pad = i32::from(key) - i32::from(atomik_devices::drum_machine::PAD_KEY_BASE);
        if (0..i32::from(DRUM_PADS)).contains(&pad) {
            counts[pad as usize] += 1;
        }
    }
    let (best, n) = counts
        .iter()
        .enumerate()
        .max_by_key(|(i, n)| (**n, usize::MAX - i))?;
    (*n > 0).then_some(best as u8 + 1)
}

/// Whether `device` of `doc` is of one of `types`.
fn is_type(doc: &RackDoc, device: &DeviceKey, types: &[&str]) -> bool {
    doc.resolve(device)
        .is_some_and(|d| types.contains(&d.type_id.as_str()))
}

/// Whether a check has anything to fix: a hit that cancels, or two fundamentals that beat.
pub fn needs_fix(check: &Check) -> bool {
    check.cancelling() > 0 || check.beating
}

/// The steps of the whole analysis, for the progress bar: the base render, then at most this
/// many trial renders.
const MAX_STEPS: usize = 1 + 1 + 4 + 1 + 1 + 1 + 4;

/// Runs the analysis of `setup` on `doc` (see the module docs). `progress` is told how far it
/// is, 0 to 1; `cancel` stops it (the result is then `Ok(None)`).
///
/// # Errors
/// The render fails, or a track makes no sound (the text says which).
pub fn analyse(
    doc: &RackDoc,
    registry: &Registry,
    bundle_root: Option<&Path>,
    setup: &Setup,
    (cancel, progress): (&AtomicBool, &dyn Fn(f32)),
) -> Result<Option<Analysis>, String> {
    let mut done = 0usize;
    let mut tick = |n: usize| {
        done += n;
        progress((done as f32 / MAX_STEPS as f32).min(1.0));
    };
    let stopped = || cancel.load(std::sync::atomic::Ordering::Relaxed);
    let Some(pair) = render_pair(doc, registry, bundle_root, setup, Some(cancel))? else {
        return Ok(None);
    };
    tick(1);
    let name = |key: &DeviceKey| {
        doc.device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    };
    for (signal, track) in [(&pair.kick, &setup.kick), (&pair.bass, &setup.bass)] {
        let power =
            signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len().max(1) as f64;
        if power < kick_bass::FLOOR_POWER {
            return Err(format!(
                "'{}' makes no sound in the first {CHECK_SECONDS:.0} seconds (a muted track, \
                 an empty track or a plugin does not count)",
                name(track)
            ));
        }
    }
    let rate = pair.sample_rate;
    let base = kick_bass::check(&pair.kick, &pair.bass, rate);
    let mut analysis = Analysis {
        setup: setup.clone(),
        base: base.clone(),
        trials: Vec::new(),
        notes: Vec::new(),
        headroom: None,
    };
    // The fix-asymmetry helper: the angle that evens out the peaks of the bass, proven by a render.
    let bass_hz = base.bass_hz.unwrap_or(FALLBACK_HZ);
    let hz = bass_hz.clamp(20.0, 1000.0).round();
    let found = atomik_dsp::best_rotation(&pair.bass, rate as f32, hz);
    if found.angle_deg > 0.0 {
        let fix = Fix::Rotate {
            degrees: found.angle_deg,
            hz,
        };
        let mut scratch = Scratch {
            document: Document::new(doc.clone()),
            registry,
        };
        if apply_fix(&mut scratch, setup, &fix, rate).is_ok() {
            let Some(fixed) = render_pair(
                scratch.document.doc(),
                registry,
                bundle_root,
                setup,
                Some(cancel),
            )?
            else {
                return Ok(None);
            };
            analysis.headroom = Some(Headroom {
                found,
                trial: Trial {
                    fix,
                    check: kick_bass::check(&fixed.kick, &fixed.bass, rate),
                },
            });
        }
    }
    tick(1);
    if base.hits.is_empty() {
        analysis
            .notes
            .push("No kick hits were found in the first seconds of the kick track".to_owned());
        return Ok(Some(analysis));
    }
    if !needs_fix(&base) {
        return Ok(Some(analysis));
    }
    let mut candidates: Vec<Vec<Fix>> = Vec::new();
    if is_type(doc, &setup.bass, &PHASE_TYPES) {
        candidates.push(
            START_ANGLES
                .iter()
                .map(|&degrees| Fix::StartPhase { degrees })
                .collect(),
        );
    } else {
        analysis
            .notes
            .push("The bass is not a synth with a start phase".to_owned());
    }
    candidates.push(vec![Fix::Polarity]);
    // The rotation and the delay are searched on the signals, then proven by a render.
    let best_rotation = (5..180)
        .step_by(5)
        .map(|a| {
            let degrees = a as f32;
            let mut r = atomik_dsp::PhaseRotator::new();
            r.set(degrees, bass_hz, rate as f32);
            let turned: Vec<f32> = pair.bass.iter().map(|&s| r.process(s)).collect();
            (degrees, kick_bass::check(&pair.kick, &turned, rate).score())
        })
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.total_cmp(&a.0)));
    if let Some((degrees, score)) = best_rotation
        && score >= base.score() + MIN_GAIN
    {
        candidates.push(vec![Fix::Rotate {
            degrees,
            hz: bass_hz.clamp(20.0, 1000.0).round(),
        }]);
    }
    if stopped() {
        return Ok(None);
    }
    let period = (f64::from(rate) / f64::from(bass_hz)).ceil() as usize;
    if let Some(frames) = kick_bass::best_bass_delay(&pair.kick, &pair.bass, rate, period) {
        candidates.push(vec![Fix::Delay {
            frames: frames as u32,
        }]);
    }
    if is_type(doc, &setup.kick, &[DRUM_TYPE]) {
        match kick_pad(doc, &setup.kick) {
            Some(pad) => candidates.push(
                NUDGES
                    .iter()
                    .map(|&cents| Fix::KickPitch { pad, cents })
                    .collect(),
            ),
            None => analysis
                .notes
                .push("The kick track plays no pad of its drum machine".to_owned()),
        }
    }
    for group in candidates {
        let mut best: Option<Trial> = None;
        for fix in group {
            if stopped() {
                return Ok(None);
            }
            let mut scratch = Scratch {
                document: Document::new(doc.clone()),
                registry,
            };
            // A fix the song cannot take (a missing device) is skipped.
            if apply_fix(&mut scratch, setup, &fix, rate).is_err() {
                continue;
            }
            let Some(fixed) = render_pair(
                scratch.document.doc(),
                registry,
                bundle_root,
                setup,
                Some(cancel),
            )?
            else {
                return Ok(None);
            };
            tick(1);
            let check = kick_bass::check(&fixed.kick, &fixed.bass, rate);
            if best
                .as_ref()
                .is_none_or(|b| check.score() > b.check.score())
            {
                best = Some(Trial { fix, check });
            }
        }
        if let Some(trial) = best
            && trial.check.score() >= base.score() + MIN_GAIN
        {
            analysis.trials.push(trial);
        }
    }
    analysis
        .trials
        .sort_by(|a, b| b.check.score().total_cmp(&a.check.score()));
    progress(1.0);
    Ok(Some(analysis))
}

#[cfg(test)]
pub(crate) mod tests;
