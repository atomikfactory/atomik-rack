//! Learn from kick: the LEARN button of the Sidechain Tool turns the hits in what the device
//! listens to into a drawn curve.
//!
//! * **Listening.** The range between the locators (at most 30 seconds from its start) is rendered
//!   offline with the same renderer as a bounce, on the worker `atomik-duck-learn`, with one tap
//!   on whatever feeds the device's key: its `sc_in_l` and `sc_in_r` when a cable is in either,
//!   else its `in_l` and `in_r` ([`atomik_render::device_input::render_device_key`]). The device
//!   hears exactly what it hears in playback, and its own settings change nothing in it. The
//!   worker never touches the song or the audio thread. Plugins sound as silent stand-ins when the
//!   sandbox is off, so a project that relies on them may find less.
//! * **Learning.** [`atomik_dsp::duck_learn::learn`] finds the hits, averages their envelopes over
//!   the device's curve `length` (a note value or a bar of the song at the start of the range, at
//!   the tempo there) and simplifies the average to at most eight points with bends.
//! * **Applying.** The points, their number and the curve mode go into the device in **one undo
//!   step** (a `Command::Batch` of parameter changes), and the status line says what was learned.
//!   Until the answer arrives nothing in the song changes, a search that finds nothing changes
//!   nothing, and a second press while one runs is refused.
//!
//! A running search belongs to one project: opening or creating another one stops it (its
//! answer is ignored).

use super::*;
use atomik_devices::duck::curve_length_ticks;
use atomik_dsp::duck_curve::{COUNT_KEY, POINT_KEYS};
use atomik_dsp::duck_learn::{Learned, learn};
use atomik_render::device_input::render_device_key;
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The type id of the Sidechain Tool.
const DUCK: &str = "atomik.duck";
/// The longest stretch of sound a search listens to, in seconds.
const MAX_SECONDS: f64 = 30.0;
/// The value of the `mode` param that plays the drawn curve.
const CURVE_MODE: f32 = 3.0;

/// What a search sends back: the curve or why not.
type Answer = Result<Learned, String>;

/// A running search.
#[derive(Debug)]
struct Running {
    device: DeviceKey,
    generation: u64,
    cancel: Arc<AtomicBool>,
    answer: Receiver<Answer>,
}

/// The learner's state in the app core.
#[derive(Debug, Default)]
pub(super) struct Learner {
    running: Option<Running>,
}

/// What a search needs from the project, taken before the worker starts.
struct Work {
    doc: RackDoc,
    device: DeviceKey,
    span: (u64, u64),
    window_ms: f32,
}

/// The search itself: renders the span and learns the curve from it.
fn search(
    work: &Work,
    bundle_root: Option<&Path>,
    plugins: &crate::freeze_worker::PluginEnv,
    cancel: &AtomicBool,
) -> Answer {
    let mut registry = atomik_render::default_registry();
    let mut host = crate::freeze_worker::plugin_host(
        &work.doc,
        plugins.clone(),
        &mut registry,
        "learn from kick",
    )
    .unwrap_or(None);
    atomik_model::register_stand_ins(&work.doc, &mut registry).map_err(|e| e.to_string())?;
    let options = ProjectOptions {
        host_block: std::num::NonZeroUsize::new(512).unwrap_or(std::num::NonZeroUsize::MIN),
        workers: 0,
        loop_count: None,
    };
    let mut bounce = PluginBounce {
        host: host.as_mut(),
        mode: atomik_device_sdk::RunMode::Offline,
        missing: MissingPlugins::StandIn,
    };
    let control = RenderControl {
        cancel: Some(cancel),
        progress: None,
    };
    let key = render_device_key(
        &work.doc,
        &registry,
        (&options, bundle_root),
        &mut bounce,
        &work.device,
        work.span,
        control,
    )
    .map_err(|e| e.to_string())?;
    let Some(key) = key else {
        return Err("it was stopped".to_owned());
    };
    #[allow(clippy::cast_precision_loss)] // a sample rate
    let rate = key.sample_rate as f32;
    learn(&key.left, &key.right, rate, work.window_ms).map_err(|e| e.to_string())
}

impl<S: EngineSink> AppCore<S> {
    fn is_duck(&self, device: &DeviceKey) -> bool {
        self.doc().device(device).is_some_and(|d| d.type_id == DUCK)
    }

    /// Starts a search for `device` (module docs). A refusal goes to the status line.
    pub fn learn_duck(&mut self, device: &DeviceKey) {
        if !self.is_duck(device) {
            self.reject("Learn from kick works on a Sidechain Tool".to_owned());
            return;
        }
        if self.duck_learn.running.is_some() {
            self.reject("Learn from kick is already running".to_owned());
            return;
        }
        let doc = self.doc();
        let (start, end) = doc.song.loop_range;
        if start >= end {
            self.reject("The locators are at the same place: move them apart".to_owned());
            return;
        }
        let rate = doc.sample_rate;
        let from = doc.song.frame_at(start, rate);
        let limit = from + (MAX_SECONDS * f64::from(rate)).round() as u64;
        let to = doc.song.frame_at(end, rate).min(limit);
        if to <= from {
            self.reject("There is nothing between the locators to listen to".to_owned());
            return;
        }
        // The curve is as long as the device's `length` at the start of the range.
        let length = doc
            .device(device)
            .and_then(|d| d.params.get("length"))
            .map_or(2, |v| v.round().clamp(0.0, 4.0) as u8);
        let ticks = curve_length_ticks(length, doc.song.bar_ticks_at(start) as f64);
        let curve_end = start + ticks.round() as i64;
        let frames = doc.song.frame_at(curve_end, rate) - from;
        let window_ms = (frames as f64 * 1000.0 / f64::from(rate)) as f32;
        let work = Work {
            doc: doc.clone(),
            device: device.clone(),
            span: (from, to),
            window_ms,
        };
        let root = self.bundle_root().map(Path::to_path_buf);
        let env = crate::freeze_worker::PluginEnv {
            cache: self.plugins.cache.clone(),
            settings: self.plugins.settings.clone(),
            isolation: self.plugins.freeze_isolation.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, answer) = channel();
        let flag = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("atomik-duck-learn".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    search(&work, root.as_deref(), &env, &flag)
                }))
                .unwrap_or_else(|_| Err("a device failed while the sound rendered".to_owned()));
                let _ = send.send(result);
            });
        if let Err(e) = spawned {
            self.reject(format!("Learn from kick: cannot start the search: {e}"));
            return;
        }
        self.duck_learn.running = Some(Running {
            device: device.clone(),
            generation: self.audio.generation,
            cancel,
            answer,
        });
        self.report("Learn from kick: listening to the range between the locators\u{2026}");
    }

    /// One frame: takes the answer of a finished search and applies it.
    pub(super) fn poll_duck_learn(&mut self) {
        let generation = self.audio.generation;
        let Some(run) = &self.duck_learn.running else {
            return;
        };
        let device = run.device.clone();
        let stale = run.generation != generation;
        let answer = match run.answer.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) if stale => {
                run.cancel.store(true, Ordering::Relaxed);
                return;
            }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the search stopped unexpectedly".to_owned()),
        };
        self.duck_learn.running = None;
        if stale {
            return;
        }
        match answer {
            Ok(learned) => self.apply_learned(&device, &learned),
            Err(why) => self.reject(format!("Learn from kick: {why}")),
        }
    }

    /// Puts a learned curve into `device`: the mode, the number of points and the points, in one
    /// undo step.
    fn apply_learned(&mut self, device: &DeviceKey, learned: &Learned) {
        if !self.is_duck(device) {
            return;
        }
        let set = |param: &str, value: f32| Command::SetParam {
            key: device.clone(),
            param: param.to_owned(),
            value,
            merge: None,
        };
        let mut commands = vec![
            set("mode", CURVE_MODE),
            set(COUNT_KEY, learned.points.len() as f32),
        ];
        for (keys, p) in POINT_KEYS.iter().zip(&learned.points) {
            commands.push(set(keys[0], p.x));
            commands.push(set(keys[1], p.y));
            commands.push(set(keys[2], f32::from(p.bend)));
        }
        if !self.apply_batch(commands) {
            return;
        }
        self.report(format!(
            "Learn from kick: {} hit{} averaged into a curve of {} points \u{b7} the mode is now curve",
            learned.hits,
            if learned.hits == 1 { "" } else { "s" },
            learned.points.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::RecordingSink;
    use atomik_core::PPQ;
    use atomik_model::song::ClipId;
    use atomik_model::{
        Clip, ClipContent, ConsoleCommand, CreateOptions, CreateSource, InsertEffect, Note,
        SongCommand, WorkflowCommand, new_project_doc,
    };

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    fn set(core: &mut AppCore<RecordingSink>, device: &DeviceKey, param: &str, value: f32) {
        core.apply_command(Command::SetParam {
            key: device.clone(),
            param: param.into(),
            value,
            merge: None,
        })
        .unwrap();
    }

    /// A sine synth with a short decay playing a note on every beat of four bars (a kick), on a
    /// channel with a Sidechain Tool insert; the locators span the four bars (8 s at 120 BPM).
    /// `notes` says whether the clip has them. Returns the core and the tool's key.
    fn core_with_kick(notes: bool) -> (AppCore<RecordingSink>, DeviceKey) {
        let mut core = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
        core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key("kick")),
            options: CreateOptions::default(),
        }))
        .unwrap();
        for (p, v) in [
            ("osc1.wave", 3.0),
            ("osc.mix", 0.0),
            ("voices", 1.0),
            ("env.amp.attack", 1.0),
            ("env.amp.decay", 90.0),
            ("env.amp.sustain", 0.0),
            ("env.amp.release", 20.0),
        ] {
            set(&mut core, &key("kick"), p, v);
        }
        let hits: Vec<Note> = if notes {
            (0..16)
                .map(|b| Note::new(b * PPQ, PPQ / 4, 36, 0.9))
                .collect()
        } else {
            Vec::new()
        };
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key("kick"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 16 * PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                param_lanes: Vec::new(),
                midi: Default::default(),
                groove: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(hits.into()),
            },
        }))
        .unwrap();
        let channel = core
            .doc()
            .console
            .channels
            .iter()
            .find(|c| c.source.as_ref() == Some(&key("kick")))
            .unwrap()
            .channel
            .clone();
        core.apply_command(Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: DUCK.into(),
            }),
        }))
        .unwrap();
        let duck = core
            .doc()
            .devices
            .iter()
            .find(|d| d.type_id == DUCK)
            .unwrap()
            .key
            .clone();
        (core, duck)
    }

    fn wait_for_answer(core: &mut AppCore<RecordingSink>) {
        for _ in 0..4000 {
            core.frame();
            if core.duck_learn.running.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the search did not finish");
    }

    fn said(core: &AppCore<RecordingSink>, text: &str) -> bool {
        core.last_rejection().is_some_and(|m| m.contains(text))
    }

    #[test]
    fn learning_puts_the_kick_curve_into_the_device_in_one_undo_step() {
        let (mut core, duck) = core_with_kick(true);
        let before = core.doc().clone();
        core.learn_duck(&duck);
        assert!(core.duck_learn.running.is_some());
        // Nothing changes while it listens, and a second press is refused.
        assert!(core.doc() == &before);
        core.learn_duck(&duck);
        assert!(said(&core, "already running"));
        wait_for_answer(&mut core);
        let device = core.doc().device(&duck).unwrap().clone();
        let at = |k: &str| device.params.get(k).copied().unwrap_or(0.0);
        assert!(
            said(&core, "Learn from kick"),
            "{:?}",
            core.last_rejection()
        );
        assert!((at("mode") - 3.0).abs() < f32::EPSILON);
        let n = at(COUNT_KEY) as usize;
        assert!((3..=8).contains(&n), "{n}");
        // The curve starts at its left edge, ends at its right edge and released, and peaks
        // early (a 90 ms decay in a quarter note of 500 ms).
        assert!(at(POINT_KEYS[0][0]).abs() < f32::EPSILON);
        assert!((at(POINT_KEYS[n - 1][0]) - 1.0).abs() < f32::EPSILON);
        assert!(at(POINT_KEYS[n - 1][1]).abs() < f32::EPSILON);
        let top = (0..n)
            .map(|i| (at(POINT_KEYS[i][1]), at(POINT_KEYS[i][0])))
            .fold((0.0f32, 0.0f32), |a, b| if b.0 > a.0 { b } else { a });
        assert!(top.0 > 0.8 && top.1 < 0.3, "{top:?}");
        // One undo puts everything back.
        core.undo();
        assert!(core.doc() == &before);
    }

    #[test]
    fn the_curve_length_follows_the_devices_length() {
        let learn_with = |length: f32| {
            let (mut core, duck) = core_with_kick(true);
            set(&mut core, &duck, "length", length);
            core.learn_duck(&duck);
            wait_for_answer(&mut core);
            core.doc().device(&duck).unwrap().clone()
        };
        // A sixteenth note (125 ms) holds a shorter part of the kick than a half note (1 s), so
        // the top of the curve is farther along it.
        let (short, long) = (learn_with(0.0), learn_with(3.0));
        let top_x = |d: &atomik_model::RackDevice| {
            let n = d.params[COUNT_KEY] as usize;
            (0..n)
                .map(|i| (d.params[POINT_KEYS[i][1]], d.params[POINT_KEYS[i][0]]))
                .fold((0.0f32, 0.0f32), |a, b| if b.0 > a.0 { b } else { a })
                .1
        };
        assert!(
            top_x(&short) > top_x(&long),
            "{} {}",
            top_x(&short),
            top_x(&long)
        );
    }

    #[test]
    fn other_devices_are_refused_and_so_is_a_sound_with_no_hit() {
        let (mut core, duck) = core_with_kick(false);
        core.learn_duck(&key("kick"));
        assert!(said(&core, "Sidechain Tool"));
        assert!(core.duck_learn.running.is_none());
        let before = core.doc().clone();
        core.learn_duck(&duck);
        wait_for_answer(&mut core);
        assert!(said(&core, "too quiet"), "{:?}", core.last_rejection());
        assert!(core.doc() == &before);
    }
}
