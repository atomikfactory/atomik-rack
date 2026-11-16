//! The tonal balance meter (`atomik.tonal-balance`) in the app core.
//!
//! # Live reading
//!
//! Once a frame, for every tonal balance panel that is on screen ([`atomik_ui::RackView::visible_panels`]:
//! not folded, not scrolled out, not on the back face) and whose device is not bypassed, the core
//! reads the newest 16 384 frames of the device's scope tap ([`EngineSink::read_device_scope`],
//! lock-free; the tap is filled by the audio thread with a plain copy, always), runs the FFT of
//! the spectrum analyser ([`atomik_dsp::spectrum`]: Hann window, the `channel`, a one-pole
//! average over the `response` time measured in audio frames) down to the 31 one-third-octave
//! band levels and compares them with the target ([`atomik_dsp::tonal`]). The comparison is
//! handed to the rack view as a [`TonalView`] inside the device's
//! [`atomik_ui::analyser::AnalyserData`]. A tap that did not move since the last frame (a stopped
//! engine) is not analysed again, but the comparison is made again every frame, so a new target
//! shows at once. The FFT never runs on the audio thread, and a panel that is not shown costs
//! nothing.
//!
//! # Targets
//!
//! The device's `hidden.target` param says which target: 0 to 3 the built-ins, 4 the learned one
//! not saved yet (kept in the session), `10 + id` a saved one. The saved targets are the user's
//! ([`crate::tonal_targets`], `tonal-targets.json` next to the library file). A value that names
//! nothing (a saved target that is not here, a learned one after a restart) reads as Balanced and
//! the view says the target is missing.
//!
//! # Learning
//!
//! * **Live.** While a device's `learn` is on, every two seconds of audio the current band
//!   levels are taken as an observation ([`atomik_dsp::tonal::Learner`]). When it goes off the
//!   target they make becomes the learned target, and the device points at it.
//! * **From files.** [`AppCore::learn_tonal_files`] decodes the files and analyses them on a
//!   worker thread (`atomik-tonal-files`, off the audio thread and the UI, with progress and
//!   cancelling on drop), each in stretches of about three seconds into one learner, and the
//!   finished target is the learned one.
//! * **Saving.** [`TonalOp::Save`] keeps the learned target under a name, [`TonalOp::Rename`] and
//!   [`TonalOp::Delete`] work on the saved target in force; every change is written to the file
//!   (unless a newer version wrote it, in which case it stays in the session).

use super::*;
use crate::tonal_targets::{self, Store};
use atomik_audio_file::decode_file_with;
use atomik_devices::tonal_balance::{RESPONSE_MS, TARGET_LEARNED, TARGET_USER_BASE};
use atomik_dsp::spectrum::{
    BAND_COUNT, ChannelMode, Settings, Smoothing, Spectrum, Weighting, WindowKind,
};
use atomik_dsp::tonal::{
    BUILTIN_NAMES, FFT_SIZE, LIVE_OBSERVATION_S, Learner, Target, builtin, compare,
    learn_from_channels,
};
use atomik_ui::analyser::AnalyserData;
use atomik_ui::fx::FxKind;
use atomik_ui::tonal_panel::{TargetKind, TargetRow, TonalOp, TonalView};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Largest file decoded for learning, in samples over all channels (about 15 minutes of stereo
/// at 96 kHz).
const MAX_FILE_SAMPLES: u64 = 180_000_000;
/// The device param the panel's target list sets.
const TARGET_PARAM: &str = "hidden.target";

/// What the analysis of the files made.
struct Done {
    target: Target,
    files: usize,
    skipped: usize,
}

/// A learn from files that runs on a worker.
struct Job {
    device: DeviceKey,
    result: Receiver<Result<Done, String>>,
    /// Thousandths done.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The reading state of one tonal balance device.
struct Live {
    id: DeviceInstanceId,
    spectrum: Spectrum,
    /// The tap head the levels were computed at.
    head: u64,
    fed: bool,
    levels: Option<[f32; BAND_COUNT]>,
    learner: Learner,
    learning: bool,
    /// Audio frames taken in by the live learn.
    learned_frames: u64,
    /// Audio frames since the last observation.
    since_observation: u64,
}

impl Live {
    fn new(id: DeviceInstanceId) -> Self {
        Self {
            id,
            spectrum: Spectrum::new(),
            head: 0,
            fed: false,
            levels: None,
            learner: Learner::new(),
            learning: false,
            learned_frames: 0,
            since_observation: 0,
        }
    }
}

/// The tonal balance state of the core.
#[derive(Default)]
pub(super) struct Tonal {
    store: Store,
    path: Option<PathBuf>,
    /// The file was written by a newer version or cannot be read: never overwrite it.
    locked: bool,
    /// The learned target waiting to be saved (the `hidden.target` value 4).
    pub(super) pending: Option<Target>,
    live: BTreeMap<DeviceKey, Live>,
    job: Option<Job>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Tonal {
    /// A new engine knows nothing of the taps: they start from frame 0.
    pub(super) fn restart(&mut self) {
        self.live.clear();
    }
}

/// The target a `hidden.target` value names, with its name and whether the value named nothing
/// (so Balanced stands in).
pub fn resolve_target(
    store: &Store,
    pending: Option<&Target>,
    value: f32,
) -> (Target, String, TargetKind, bool) {
    let v = if value.is_finite() {
        value.round()
    } else {
        0.0
    };
    let balanced = || {
        (
            builtin(0),
            BUILTIN_NAMES[0].to_string(),
            TargetKind::Builtin,
            true,
        )
    };
    if (0.0..BUILTIN_NAMES.len() as f32).contains(&v) {
        let i = v as usize;
        return (
            builtin(i),
            BUILTIN_NAMES[i].to_string(),
            TargetKind::Builtin,
            false,
        );
    }
    if v == TARGET_LEARNED {
        return match pending {
            Some(t) => (t.clone(), "Learned".to_string(), TargetKind::Learned, false),
            None => balanced(),
        };
    }
    if v >= TARGET_USER_BASE {
        let id = (v - TARGET_USER_BASE) as u32;
        if let Some(s) = store.get(id) {
            return (s.target.clone(), s.name.clone(), TargetKind::User, false);
        }
    }
    balanced()
}

/// The rows of the target list: the built-ins, the learned one if there is one, the saved ones.
pub fn target_rows(store: &Store, pending: bool) -> Vec<TargetRow> {
    let mut rows: Vec<TargetRow> = BUILTIN_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| TargetRow {
            value: i as f32,
            name: (*name).to_string(),
            kind: TargetKind::Builtin,
        })
        .collect();
    if pending {
        rows.push(TargetRow {
            value: TARGET_LEARNED,
            name: "Learned".to_string(),
            kind: TargetKind::Learned,
        });
    }
    rows.extend(store.targets.iter().map(|s| TargetRow {
        value: TARGET_USER_BASE + s.id as f32,
        name: s.name.clone(),
        kind: TargetKind::User,
    }));
    rows
}

/// The settings a device's params ask of the analysis.
struct Params {
    response: usize,
    channel: ChannelMode,
    learn: bool,
    bypass: bool,
    target: f32,
}

fn params(desc: &DeviceDescriptor, dev: &RackDevice) -> Params {
    let value = |k: &str| {
        desc.params
            .iter()
            .find(|p| p.key == k)
            .map_or(0.0, |p| atomik_ui::widget::current_value(dev, p))
    };
    Params {
        response: (value("response").round().max(0.0) as usize).min(RESPONSE_MS.len() - 1),
        channel: match value("channel").round() as i32 {
            1 => ChannelMode::Mid,
            2 => ChannelMode::Side,
            _ => ChannelMode::Sum,
        },
        learn: value("learn") >= 0.5,
        bypass: value("bypass") >= 0.5,
        target: value(TARGET_PARAM),
    }
}

fn settings(p: &Params) -> Settings {
    Settings {
        size: FFT_SIZE,
        window: WindowKind::Hann,
        channel: p.channel,
        bands: true,
        averaging_ms: RESPONSE_MS[p.response],
        hold_ms: 0.0,
        decay_db_per_s: 0.0,
        tilt_db_per_oct: 0.0,
        smoothing: Smoothing::Off,
        weighting: Weighting::Flat,
    }
}

/// What the end of a live learn leaves to do once the state is back in the core.
struct Finished {
    device: DeviceKey,
    target: Option<Target>,
    seconds: f32,
}

impl<S: EngineSink> AppCore<S> {
    /// Where the saved targets live (`tonal-targets.json` next to the library file); `None`
    /// keeps them in memory only (tests). Reads the file now: a damaged one is ignored until the
    /// next save, one from a newer version is never overwritten.
    pub fn set_tonal_path(&mut self, path: Option<PathBuf>) {
        use crate::tonal_targets::{Loaded, load};
        let t = &mut self.tonal;
        t.store = Store {
            next_id: 1,
            ..Store::default()
        };
        t.locked = false;
        if let Some(p) = &path {
            match load(p) {
                Loaded::Store(store) => t.store = store,
                Loaded::Missing => {}
                Loaded::Unusable { why, newer } => {
                    log::warn!(
                        "the saved tonal balance targets {} are not used: {why}",
                        p.display()
                    );
                    t.locked = newer;
                }
            }
        }
        t.path = path;
    }

    /// The saved tonal balance targets.
    pub fn tonal_targets(&self) -> &Store {
        &self.tonal.store
    }

    /// The learned target that waits to be saved.
    pub fn tonal_pending(&self) -> Option<&Target> {
        self.tonal.pending.as_ref()
    }

    /// Whether a learn from files is running.
    pub fn tonal_job_running(&self) -> bool {
        self.tonal.job.is_some()
    }

    /// Reads the taps of the tonal balance panels that are shown and feeds the rack view (module
    /// docs); once per frame from [`Self::frame`].
    pub(super) fn feed_tonal(&mut self) {
        self.poll_tonal_job();
        let shown = self.view.visible_panels(FxKind::TonalBalance);
        if shown.is_empty() {
            return;
        }
        let sample_rate = self.doc().sample_rate as f32;
        let mut state = std::mem::take(&mut self.tonal);
        state.left.resize(FFT_SIZE, 0.0);
        state.right.resize(FFT_SIZE, 0.0);
        let mut finished: Vec<Finished> = Vec::new();
        for key in shown {
            let Some(id) = self.ids.get(&key).copied() else {
                continue;
            };
            let Some(p) = self.doc().resolve(&key).and_then(|dev| {
                self.registry
                    .descriptor(&dev.type_id)
                    .map(|desc| params(desc, dev))
            }) else {
                continue;
            };
            if p.bypass {
                continue;
            }
            let live = state
                .live
                .entry(key.clone())
                .or_insert_with(|| Live::new(id));
            if live.id != id {
                *live = Live::new(id);
            }
            let read = self
                .sink
                .read_device_scope(id, &mut state.left, &mut state.right);
            if let Some((n, head)) = read
                && head != 0
                && !(head == live.head && live.fed)
            {
                let elapsed = if live.fed {
                    head.saturating_sub(live.head)
                } else {
                    0
                };
                live.head = head;
                live.fed = true;
                live.spectrum.update(
                    &state.left[..n],
                    &state.right[..n],
                    elapsed,
                    sample_rate,
                    &settings(&p),
                    0,
                );
                let levels = live.spectrum.band_levels();
                live.levels = levels
                    .iter()
                    .any(|v| *v > atomik_dsp::spectrum::FLOOR_DB + 50.0)
                    .then_some(levels);
                if live.learning && p.learn {
                    live.learned_frames += elapsed;
                    live.since_observation += elapsed;
                    if live.since_observation as f32 >= LIVE_OBSERVATION_S * sample_rate {
                        live.since_observation = 0;
                        if let Some(levels) = &live.levels {
                            live.learner.push(levels);
                        }
                    }
                }
            }
            if p.learn && !live.learning {
                live.learning = true;
                live.learner.clear();
                live.learned_frames = 0;
                live.since_observation = 0;
            } else if !p.learn && live.learning {
                live.learning = false;
                finished.push(Finished {
                    device: key.clone(),
                    target: live.learner.build(),
                    seconds: live.learned_frames as f32 / sample_rate,
                });
                live.learner.clear();
            }
            let (target, name, kind, missing) =
                resolve_target(&state.store, state.pending.as_ref(), p.target);
            let rows = target_rows(&state.store, state.pending.is_some());
            let selected = if missing {
                0
            } else {
                rows.iter()
                    .position(|r| r.kind == kind && r.name == name)
                    .unwrap_or(0)
            };
            let mut view = TonalView {
                targets: rows,
                selected,
                target_name: name,
                missing,
                learning: live.learning,
                learned_seconds: live.learned_frames as f32 / sample_rate,
                pending: state.pending.is_some(),
                files: state
                    .job
                    .as_ref()
                    .filter(|j| j.device == key)
                    .map(|j| j.progress.load(Ordering::Relaxed) as f32 / 1000.0),
                tolerance: target.region_tol,
                target_bands: target.bands.to_vec(),
                target_tol: target.tol.to_vec(),
                ..TonalView::default()
            };
            if let Some(reading) = live.levels.as_ref().and_then(|l| compare(l, &target)) {
                view.ready = true;
                view.deviation = reading.deviation;
                view.bands = reading.bands.to_vec();
            }
            let data = AnalyserData {
                tonal: view,
                ..AnalyserData::default()
            };
            self.view.set_analyser(&key, &data);
        }
        // `finished` may set `pending`: restore the state first, then act.
        self.tonal = state;
        for f in finished {
            self.finish_live_learn(f);
        }
    }

    fn finish_live_learn(&mut self, f: Finished) {
        match f.target {
            Some(target) => {
                self.tonal.pending = Some(target);
                self.set_tonal_target(&f.device, TARGET_LEARNED);
                self.report(format!(
                    "Learned a target from {:.0} s of the live signal: save it from the target list",
                    f.seconds
                ));
            }
            None => self.report("Nothing was learned: there was no signal"),
        }
    }

    /// Points the device at `value` (one undo step).
    fn set_tonal_target(&mut self, device: &DeviceKey, value: f32) {
        if let Err(e) = self.apply_command(Command::SetParam {
            key: device.clone(),
            param: TARGET_PARAM.to_string(),
            value,
            merge: None,
        }) {
            log::warn!("setting the tonal balance target: {e}");
        }
    }

    /// The `hidden.target` value of the tonal balance device `device`.
    fn tonal_value(&self, device: &DeviceKey) -> Option<f32> {
        let dev = self.doc().resolve(device)?;
        let desc = self.registry.descriptor(&dev.type_id)?;
        if desc.type_id != "atomik.tonal-balance" {
            return None;
        }
        Some(params(desc, dev).target)
    }

    /// The saved target `device` points at.
    fn tonal_saved_id(&self, device: &DeviceKey) -> Option<u32> {
        let v = self.tonal_value(device)?.round();
        if v < TARGET_USER_BASE {
            return None;
        }
        let id = (v - TARGET_USER_BASE) as u32;
        self.tonal.store.get(id).map(|_| id)
    }

    /// Writes the saved targets (module docs); says so when it fails.
    fn persist_tonal(&mut self) {
        let t = &self.tonal;
        let Some(path) = t.path.clone() else { return };
        if t.locked {
            self.report(
                "The saved targets were written by a newer version and are left alone: \
                 this change lasts for this session",
            );
            return;
        }
        if let Err(e) = tonal_targets::save(&path, &self.tonal.store) {
            log::warn!("writing {}: {e}", path.display());
            self.report(format!("The saved targets could not be written: {e}"));
        }
    }

    /// An action of the target list of the tonal balance device `device` (see
    /// [`atomik_ui::UiAction::TonalTarget`]). Learning from files needs a file dialog, which the
    /// shell opens before it calls [`Self::learn_tonal_files`].
    pub fn tonal_action(&mut self, device: &DeviceKey, op: TonalOp) {
        if self.tonal_value(device).is_none() {
            return;
        }
        match op {
            TonalOp::LearnFiles => {}
            TonalOp::Save(name) => self.save_tonal_target(device, &name),
            TonalOp::Rename(name) => {
                let Some(id) = self.tonal_saved_id(device) else {
                    self.report("Only a saved target can be renamed");
                    return;
                };
                if self.tonal.store.rename(id, &name) {
                    self.persist_tonal();
                } else {
                    self.report("That name is blank or already used by another target");
                }
            }
            TonalOp::Delete => {
                let Some(id) = self.tonal_saved_id(device) else {
                    self.report("Only a saved target can be deleted");
                    return;
                };
                let name = self.tonal.store.get(id).map(|s| s.name.clone());
                self.tonal.store.remove(id);
                self.persist_tonal();
                self.set_tonal_target(device, 0.0);
                if let Some(name) = name {
                    self.report(format!("Deleted the target {name}"));
                }
            }
        }
    }

    fn save_tonal_target(&mut self, device: &DeviceKey, name: &str) {
        let Some(target) = self.tonal.pending.clone() else {
            self.report("There is no learned target to save");
            return;
        };
        let Some(id) = self.tonal.store.add(name, target) else {
            self.report("The target could not be saved: the name is blank or there are too many");
            return;
        };
        self.tonal.pending = None;
        self.persist_tonal();
        let saved = self
            .tonal
            .store
            .get(id)
            .map_or_else(String::new, |s| s.name.clone());
        self.set_tonal_target(device, TARGET_USER_BASE + id as f32);
        self.report(format!("Saved the target {saved}"));
    }

    /// Learns a target from the audio files `paths` for the device `device` on a worker thread
    /// (module docs). Refused while another analysis runs.
    pub fn learn_tonal_files(&mut self, device: &DeviceKey, paths: Vec<PathBuf>) {
        if paths.is_empty() || self.tonal_value(device).is_none() {
            return;
        }
        if self.tonal.job.is_some() {
            self.report("Files are already being analysed");
            return;
        }
        let (tx, rx) = channel();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c) = (Arc::clone(&progress), Arc::clone(&cancel));
        let started = std::thread::Builder::new()
            .name("atomik-tonal-files".into())
            .spawn(move || {
                let _ = tx.send(analyse_files(&paths, &p, &c));
            });
        if let Err(e) = started {
            self.report(format!("Could not start the analysis: {e}"));
            return;
        }
        self.tonal.job = Some(Job {
            device: device.clone(),
            result: rx,
            progress,
            cancel,
        });
    }

    /// Takes in the result of the analysis of files, once it is there.
    fn poll_tonal_job(&mut self) {
        let Some(job) = &self.tonal.job else { return };
        let answer = match job.result.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the analysis stopped".to_string()),
        };
        let Some(job) = self.tonal.job.take() else {
            return;
        };
        match answer {
            Ok(done) => {
                self.tonal.pending = Some(done.target);
                self.set_tonal_target(&job.device, TARGET_LEARNED);
                let skipped = if done.skipped > 0 {
                    format!(" ({} skipped)", done.skipped)
                } else {
                    String::new()
                };
                self.report(format!(
                    "Learned a target from {} file{}{skipped}: save it from the target list",
                    done.files,
                    if done.files == 1 { "" } else { "s" }
                ));
            }
            Err(why) => self.report(format!("Learn from files: {why}")),
        }
    }
}

/// The worker: decodes and analyses `paths` into one target.
fn analyse_files(
    paths: &[PathBuf],
    progress: &AtomicU32,
    cancel: &AtomicBool,
) -> Result<Done, String> {
    let mut learner = Learner::new();
    let (mut files, mut skipped) = (0, 0);
    let total = paths.len() as f32;
    for (i, path) in paths.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("stopped".to_string());
        }
        let Ok(decoded) = decode_file_with(path, MAX_FILE_SAMPLES) else {
            skipped += 1;
            continue;
        };
        let audio = decoded.audio;
        let Some(left) = audio.channels.first() else {
            skipped += 1;
            continue;
        };
        let right = audio.channels.get(1).unwrap_or(left);
        let taken = learn_from_channels(
            &mut learner,
            left,
            right,
            audio.sample_rate as f32,
            |fraction| {
                progress.store(
                    (((i as f32 + fraction) / total) * 1000.0) as u32,
                    Ordering::Relaxed,
                );
                !cancel.load(Ordering::Relaxed)
            },
        );
        if taken > 0 {
            files += 1;
        } else {
            skipped += 1;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("stopped".to_string());
    }
    learner
        .build()
        .map(|target| Done {
            target,
            files,
            skipped,
        })
        .ok_or_else(|| "no usable audio in the files (too short, silent or not audio)".to_string())
}
