//! The resonance finder of the dynamic EQ: FIND turns the sound at the device's input into a
//! list of resonances, CUT turns the strongest into dynamic cuts.
//!
//! * **Find** ([`atomik_ui::UiAction::FindResonances`]). The range between the locators (at most 30
//!   seconds from its start) is rendered offline with the same renderer as a bounce, on the
//!   worker `atomik-resonance`, with one tap on whatever feeds the device's `in_l` and `in_r`
//!   ([`atomik_render::device_input::render_device_input`]): the device hears exactly what it hears
//!   in playback, and its own settings change nothing in it. The worker never touches the song or
//!   the audio thread. The captured audio goes through [`atomik_dsp::resonance::find_resonances`]
//!   (average spectrum, third-octave trend, peaks more than 6 dB above it, Q from the peak
//!   width); the list reaches the panel as markers and the status line names the frequencies.
//!   Plugins sound as silent stand-ins when the sandbox is off, so a project that relies on
//!   them may find less. A search changes nothing in the song, is not an undo step, and a
//!   second FIND while one runs is refused.
//! * **Cut** ([`atomik_ui::UiAction::CutResonances`]). The strongest resonances of the last search go into
//!   the bells of the device that are off (bands 2 to 4), one resonance per bell, as the dynamic
//!   cuts of [`atomik_devices::dynamic_eq::cuts`], in **one undo step** (a `Command::Batch` of
//!   parameter changes). A bell that is on is never touched; without a free bell, or without
//!   a search, nothing changes and the status line says why.
//!
//! The markers and the list belong to one project: opening or creating another one drops them
//! (and stops a running search, whose answer is ignored).

use super::*;
use atomik_devices::dynamic_eq::{BANDS, cuts};
use atomik_dsp::resonance::{Resonance, Settings, find_resonances};
use atomik_render::device_input::render_device_input;
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use atomik_ui::resonance_marks::ResonanceMark;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The type id of the dynamic EQ.
const DYNAMIC_EQ: &str = "atomik.dynamic-eq";
/// The longest stretch of sound a search listens to, in seconds.
const MAX_SECONDS: f64 = 30.0;

/// What a search sends back: the resonances or why not.
type Answer = Result<Vec<Resonance>, String>;

/// A running search.
#[derive(Debug)]
struct Running {
    device: DeviceKey,
    generation: u64,
    cancel: Arc<AtomicBool>,
    answer: Receiver<Answer>,
}

/// The finder's state in the app core.
#[derive(Debug, Default)]
pub(super) struct Finder {
    running: Option<Running>,
    /// The last search that finished: the device, the project generation and the list.
    found: Option<(DeviceKey, u64, Vec<Resonance>)>,
}

/// `1.2 kHz` or `310 Hz`.
fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

fn mark_of(r: &Resonance) -> ResonanceMark {
    ResonanceMark {
        freq_hz: r.freq_hz,
        q: r.q,
        prominence_db: r.prominence_db,
    }
}

/// The search itself: renders the span and finds the resonances in it.
fn search(
    doc: &RackDoc,
    device: &DeviceKey,
    (from, to): (u64, u64),
    bundle_root: Option<&Path>,
    plugins: &crate::freeze_worker::PluginEnv,
    cancel: &AtomicBool,
) -> Answer {
    let mut registry = atomik_render::default_registry();
    let mut host =
        crate::freeze_worker::plugin_host(doc, plugins.clone(), &mut registry, "the finder")
            .unwrap_or(None);
    atomik_model::register_stand_ins(doc, &mut registry).map_err(|e| e.to_string())?;
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
    let input = render_device_input(
        doc,
        &registry,
        (&options, bundle_root),
        &mut bounce,
        device,
        (from, to),
        control,
    )
    .map_err(|e| e.to_string())?;
    let Some(input) = input else {
        return Err("it was stopped".to_owned());
    };
    #[allow(clippy::cast_precision_loss)] // a sample rate
    let rate = input.sample_rate as f32;
    Ok(find_resonances(
        &input.left,
        &input.right,
        rate,
        &Settings::default(),
    ))
}

impl<S: EngineSink> AppCore<S> {
    fn is_dynamic_eq(&self, device: &DeviceKey) -> bool {
        self.doc()
            .device(device)
            .is_some_and(|d| d.type_id == DYNAMIC_EQ)
    }

    /// Starts a search for `device` (module docs). A refusal goes to the status line.
    pub fn find_resonances(&mut self, device: &DeviceKey) {
        if !self.is_dynamic_eq(device) {
            self.reject("Find Resonances works on a Dynamic EQ".to_owned());
            return;
        }
        if self.resonance.running.is_some() {
            self.reject("Find Resonances is already running".to_owned());
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
        let work = (self.doc().clone(), device.clone());
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
            .name("atomik-resonance".into())
            .spawn(move || {
                let (doc, device) = work;
                let result = catch_unwind(AssertUnwindSafe(|| {
                    search(&doc, &device, (from, to), root.as_deref(), &env, &flag)
                }))
                .unwrap_or_else(|_| Err("a device failed while the sound rendered".to_owned()));
                let _ = send.send(result);
            });
        if let Err(e) = spawned {
            self.reject(format!("Find Resonances: cannot start the search: {e}"));
            return;
        }
        self.resonance.found = None;
        self.resonance.running = Some(Running {
            device: device.clone(),
            generation: self.audio.generation,
            cancel,
            answer,
        });
        self.view.set_resonances(device, &[], true);
        self.report("Find Resonances: listening to the range between the locators\u{2026}");
    }

    /// One frame: takes the answer of a finished search, and drops what belongs to a project
    /// that was closed.
    pub(super) fn poll_resonances(&mut self) {
        let generation = self.audio.generation;
        if self
            .resonance
            .found
            .as_ref()
            .is_some_and(|(_, g, _)| *g != generation)
        {
            self.resonance.found = None;
        }
        let Some(run) = &self.resonance.running else {
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
        self.resonance.running = None;
        if stale {
            return;
        }
        match answer {
            Ok(list) => {
                let marks: Vec<ResonanceMark> = list.iter().map(mark_of).collect();
                self.view.set_resonances(&device, &marks, false);
                self.report(if list.is_empty() {
                    "Find Resonances: no resonance stands out of the trend in the range".to_owned()
                } else {
                    let names: Vec<String> = list.iter().take(6).map(|r| hz_text(r.freq_hz)).collect();
                    format!(
                        "Find Resonances: {} found ({}) \u{b7} CUT turns the strongest into dynamic cuts",
                        list.len(),
                        names.join(", ")
                    )
                });
                self.resonance.found = Some((device, generation, list));
            }
            Err(why) => {
                self.view.set_resonances(&device, &[], false);
                self.reject(format!("Find Resonances: {why}"));
            }
        }
    }

    /// Cuts the resonances of the last search into the free bells of `device` (module docs).
    pub fn cut_resonances(&mut self, device: &DeviceKey) {
        if !self.is_dynamic_eq(device) {
            self.reject("Cut works on a Dynamic EQ".to_owned());
            return;
        }
        let list = match &self.resonance.found {
            Some((d, g, list)) if d == device && *g == self.audio.generation => list.clone(),
            _ => {
                self.reject("Press FIND first: there are no resonances to cut yet".to_owned());
                return;
            }
        };
        if list.is_empty() {
            self.reject("The last search found no resonance to cut".to_owned());
            return;
        }
        let Some(dev) = self.doc().device(device) else {
            return;
        };
        let mut on = [false; BANDS];
        for (i, flag) in on.iter_mut().enumerate() {
            *flag = dev
                .params
                .get(&format!("band{}.on", i + 1))
                .is_some_and(|v| *v >= 0.5);
        }
        let free = cuts::free_bells(&on);
        if free.is_empty() {
            self.reject(
                "Every bell of this Dynamic EQ is on: switch one off to cut a resonance with it"
                    .to_owned(),
            );
            return;
        }
        let plan = cuts::plan(&list, &free);
        let commands: Vec<Command> = plan
            .iter()
            .flat_map(|(band, cut)| cut.params(*band))
            .map(|(param, value)| Command::SetParam {
                key: device.clone(),
                param,
                value,
                merge: None,
            })
            .collect();
        if !self.apply_batch(commands) {
            return;
        }
        let said: Vec<String> = plan
            .iter()
            .map(|(band, cut)| format!("band {} at {}", band + 1, hz_text(cut.freq_hz)))
            .collect();
        let left = list.len() - plan.len();
        let more = if left > 0 {
            format!(" \u{b7} {left} more found: free another bell")
        } else {
            String::new()
        };
        self.report(format!(
            "Dynamic EQ: cut {} resonance{} ({}){more}",
            plan.len(),
            if plan.len() == 1 { "" } else { "s" },
            said.join(", ")
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

    /// A sine synth holding A4 on a channel with a dynamic EQ insert; the locators span the
    /// four bars of the clip (8 s at 120 BPM). Returns the core and the EQ's key.
    fn core_with_tone() -> (AppCore<RecordingSink>, DeviceKey) {
        let mut core = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
        core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }))
        .unwrap();
        for (p, v) in [("osc1.wave", 3.0), ("osc.mix", 0.0), ("voices", 1.0)] {
            set(&mut core, &key("lead"), p, v);
        }
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key("lead"),
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
                content: ClipContent::Notes(vec![Note::new(0, 16 * PPQ, 69, 0.8)].into()),
            },
        }))
        .unwrap();
        let channel = core
            .doc()
            .console
            .channels
            .iter()
            .find(|c| c.source.as_ref() == Some(&key("lead")))
            .unwrap()
            .channel
            .clone();
        core.apply_command(Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: DYNAMIC_EQ.into(),
            }),
        }))
        .unwrap();
        let eq = core
            .doc()
            .devices
            .iter()
            .find(|d| d.type_id == DYNAMIC_EQ)
            .unwrap()
            .key
            .clone();
        (core, eq)
    }

    fn wait_for_answer(core: &mut AppCore<RecordingSink>) {
        for _ in 0..4000 {
            core.frame();
            if core.resonance.running.is_none() {
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
    fn a_search_finds_the_tone_and_shows_markers_without_touching_the_song() {
        let (mut core, eq) = core_with_tone();
        let before = core.doc().clone();
        core.find_resonances(&eq);
        assert!(core.resonance.running.is_some());
        // A second press while it runs is refused.
        core.find_resonances(&eq);
        assert!(said(&core, "already running"));
        wait_for_answer(&mut core);
        let marks = core.view.resonance_marks(&eq).to_vec();
        assert!(!marks.is_empty(), "{:?}", core.last_rejection());
        assert!(
            marks.iter().any(|m| (m.freq_hz - 440.0).abs() < 10.0),
            "{marks:?}"
        );
        assert!(said(&core, "Find Resonances"));
        // The search is not an edit.
        assert!(core.doc() == &before);
    }

    #[test]
    fn cut_makes_one_undo_step_that_sets_up_the_free_bells() {
        let (mut core, eq) = core_with_tone();
        // Nothing to cut before a search.
        core.cut_resonances(&eq);
        assert!(said(&core, "Press FIND"));
        core.find_resonances(&eq);
        wait_for_answer(&mut core);
        let before = core.doc().clone();
        core.cut_resonances(&eq);
        let after = core.doc().device(&eq).unwrap().clone();
        let at = |k: &str| after.params.get(k).copied().unwrap_or(0.0);
        assert!(
            (at("band2.on") - 1.0).abs() < f32::EPSILON,
            "{:?}",
            core.last_rejection()
        );
        assert!((at("band2.freq") - 440.0).abs() < 10.0);
        assert!(at("band2.range") < 0.0);
        assert!(at("band2.gain").abs() < f32::EPSILON);
        assert!(said(&core, "cut"));
        // One undo puts everything back.
        core.undo();
        assert!(core.doc() == &before);
    }

    #[test]
    fn a_bell_that_is_on_is_never_touched_and_no_free_bell_is_refused() {
        let (mut core, eq) = core_with_tone();
        core.find_resonances(&eq);
        wait_for_answer(&mut core);
        set(&mut core, &eq, "band2.on", 1.0);
        set(&mut core, &eq, "band2.freq", 3_000.0);
        core.cut_resonances(&eq);
        let device = core.doc().device(&eq).unwrap();
        assert!((device.params["band2.freq"] - 3_000.0).abs() < f32::EPSILON);
        assert!(
            (device.params["band3.on"] - 1.0).abs() < f32::EPSILON,
            "the next free bell"
        );
        for band in [3, 4] {
            set(&mut core, &eq, &format!("band{band}.on"), 1.0);
        }
        let before = core.doc().clone();
        core.cut_resonances(&eq);
        assert!(core.doc() == &before);
        assert!(said(&core, "Every bell"));
    }

    #[test]
    fn other_devices_are_refused() {
        let (mut core, _) = core_with_tone();
        core.find_resonances(&key("lead"));
        assert!(said(&core, "Dynamic EQ"));
        assert!(core.resonance.running.is_none());
        core.cut_resonances(&key("lead"));
        assert!(said(&core, "Dynamic EQ"));
    }

    #[test]
    fn a_silent_range_finds_nothing() {
        let (mut core, eq) = core_with_tone();
        let lead = key("lead");
        core.apply_command(Command::Song(SongCommand::SetTrackFlags {
            device: lead,
            muted: true,
            solo: false,
            armed: false,
        }))
        .unwrap();
        core.find_resonances(&eq);
        wait_for_answer(&mut core);
        assert!(core.view.resonance_marks(&eq).is_empty());
        assert!(said(&core, "no resonance"));
    }
}
