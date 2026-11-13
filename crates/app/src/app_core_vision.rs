//! The webcam controller in the app core: the camera target, the pinch-and-twist gesture that
//! turns it, the camera switch and the one-time download of the hand models.
//!
//! # The camera target
//!
//! The last control the user pressed is the **camera target**: a `(device or strip, param key)`
//! kept in view state only (never saved). It is set where every press already goes,
//! [`AppCore::param_touched`] (rack panels front and back, the parameter pane and the console
//! strips all emit [`atomik_ui::UiAction::ParamTouched`]), and by the touch detection of a
//! plugin's own window (a gesture begin, or the first change after [`TOUCH_GAP_SECS`], whether
//! or not a Learn mode is armed). Deleting the device clears it.
//!
//! # The gesture
//!
//! A pinch opens a gesture with a merge id, every twist is a `SetParam` within it, and letting go
//! closes it. That is the path a mapped controller takes ([`GestureKey`], `set_resolved`), so one
//! pinch is one undo step, Touch-mode automation writes while the fingers are together, and a
//! plugin's parameter moves like any other. A twist changes the target by `delta / full_turn` of
//! its normalised range; with fine control a hand near the camera (a big hand) moves it a quarter
//! as far, a far one all the way ([`fine_scale`]). A stepped param moves one step for each step
//! width of accumulated twist, so a hand shaking at a boundary does not make it flicker. The
//! audio thread is untouched: this is all UI-thread work on a command.
//!
//! # The two-hand pad
//!
//! Both hands pinched make the **pad** (see `atomik_vision::gesture`): the midpoint of the hands
//! is an X/Y position and the distance between them a third axis. In *Turn clicked knob* mode the
//! pad turns the last TWO clicked knobs, the older one with the left and right movement (X) and
//! the newer one with the height (Y), each from where it was when the pad began (moving the
//! hands across 80% of the picture sweeps a whole range). The whole two-hand gesture is one
//! undo step: the two `SetParam`s of a frame are one batch with one merge id. In *MIDI
//! controller* mode the pad sends controllers 24 to 26 instead (see `crate::camera_midi`).
//!
//! # Gestures that run commands
//!
//! A fist, open palm, thumbs up, point, peace or swipe may run an app command (Settings >
//! Camera, `crate::camera_gestures`): the core decides when one fires and queues its id, and the
//! shell runs it like the key bound to it ([`AppCore::take_camera_commands`]). With no row in the
//! table, nothing runs. The table, like the camera mode, may be the project's own
//! ([`AppCore::camera_override`]).
//!
//! # The camera as a MIDI controller
//!
//! The camera's mode (Settings > Camera) says what the hands do: turn the clicked knob (the
//! default), send MIDI (the camera is then an input named Camera that MIDI Learn can map to
//! anything, see `app_core_camera_midi.rs`), or both. With MIDI Learn armed the camera sends its
//! messages whatever the mode, so a control can be learned without switching.
//!
//! # The camera
//!
//! The camera is off until the user turns it on (the transport bar's CAMERA button, the
//! `camera.toggle` command or View > Camera) and off again when they turn it off, when the app
//! quits and on any camera error. With the hand models missing, turning it on opens the consent
//! dialog first; nothing is fetched before Download is pressed, and the download runs on a worker
//! with progress and cancel. Frames live in memory only (see [`crate::vision_link`]).

use super::midimap::{GestureKey, Resolved};
use super::plugins::TOUCH_GAP_SECS;
use super::*;
use crate::camera_gestures::{GestureBinding, GestureFirer};
use crate::camera_midi::{CameraMidi, gesture_of, swipe_gesture};
use crate::keymap_store::{CameraMode, CameraPrefs};
use crate::vision_link::{
    CameraEntry, DownloadProgress, FrameReport, ModelState, NoVision, Preview, VisionBackend,
    VisionConfig, VisionError, VisionSession,
};
use atomik_model::{CameraGesture, CameraOverride};
use atomik_ui::{CameraDialogState, CameraLamp, CameraPreviewState};
use atomik_vision::gesture::{GestureEvent, HandId, HandState, Pad};
use atomik_vision::tracker::TrackerEvent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What the indicator says when a pinch finds no target.
pub const NO_TARGET_MESSAGE: &str = "Click a knob first";

/// What the indicator says when a two-hand pad finds fewer than two knobs.
pub const NO_PAD_TARGETS_MESSAGE: &str = "Click two knobs first";

/// How much of the picture the hands cross, in one direction, to sweep a whole range with the
/// pad.
const PAD_RANGE: f32 = 0.8;

/// The hand size (in frame heights) at or below which a hand counts as far: full range.
const FAR_SIZE: f32 = crate::camera_midi::FAR_SIZE;
/// The hand size at or above which a hand counts as near: a quarter of the range.
const NEAR_SIZE: f32 = crate::camera_midi::NEAR_SIZE;
/// The smallest scale fine control gives a move.
const FINE_SCALE: f32 = 0.25;

/// How much of the range one turn moves, given the hand size at the pinch: 1 for a far hand,
/// [`FINE_SCALE`] for a near one, in between linearly. Always 1 with fine control off.
pub fn fine_scale(fine: bool, depth: f32) -> f32 {
    if !fine || !depth.is_finite() {
        return 1.0;
    }
    let t = ((depth - FAR_SIZE) / (NEAR_SIZE - FAR_SIZE)).clamp(0.0, 1.0);
    1.0 - t * (1.0 - FINE_SCALE)
}

/// One pinch in progress.
#[derive(Debug, Clone)]
struct Pinch {
    hand: HandId,
    /// The merge id of its gesture (`None` while it has no target to move).
    merge: Option<u64>,
    /// The normalised value at the pinch.
    start: f32,
    /// The plain value at the pinch (a stepped param counts steps from it).
    start_plain: f32,
    /// The scaled turn since the pinch, as a fraction of the range, kept inside the range.
    moved: f32,
    /// The steps a stepped param has moved from where the pinch began (held with hysteresis).
    step: i32,
    /// The scale of fine control (`None` until the hand's size is known).
    scale: Option<f32>,
    /// Turn the hand made before its size was known, in radians.
    pending: f32,
}

/// One two-hand pad in progress.
#[derive(Debug, Clone)]
struct PadDrag {
    /// The merge id of its gesture (`None` while it has fewer than two knobs to move).
    merge: Option<u64>,
    /// The two knobs, X then Y.
    targets: Option<[(DeviceKey, String); 2]>,
    /// The normalised values of the knobs at the start.
    start: [f32; 2],
    /// Where the midpoint of the hands was when it began (known from the first frame).
    origin: Option<(f32, f32)>,
}

/// The download of the models on its worker.
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Option<DownloadProgress>>>,
    result: Arc<Mutex<Option<Result<(), VisionError>>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// What the app keeps for the webcam controller (none of it is saved with a project).
pub(super) struct VisionCore {
    backend: Arc<dyn VisionBackend>,
    session: Option<Box<dyn VisionSession>>,
    prefs: CameraPrefs,
    target: Option<(DeviceKey, String)>,
    /// The control clicked before the target (the pad's X).
    previous: Option<(DeviceKey, String)>,
    pinch: Option<Pinch>,
    /// The two-hand pad in progress.
    pad: Option<PadDrag>,
    /// The pad of the last frame report (for the preview's crosshair).
    pad_now: Option<Pad>,
    /// Decides which gesture runs which command.
    firer: GestureFirer,
    /// The commands the gestures ran, for the shell to run.
    commands: Vec<String>,
    /// The last state of each hand followed.
    hands: BTreeMap<HandId, HandState>,
    /// When each plugin param last changed (the touch of a plugin that sends no gesture begin).
    seen: BTreeMap<(DeviceKey, u32), f64>,
    dialog: Option<CameraDialogState>,
    job: Option<Job>,
    message: Option<String>,
    lamp: CameraLamp,
    fps: f32,
    infer_ms: f32,
    preview: Option<Preview>,
    /// The preview box is shown (it needs the camera on to have a picture).
    preview_on: bool,
    /// What the hands send as MIDI.
    midi: CameraMidi,
    /// Routes the camera's notes to the selected instrument when the settings say so.
    notes: crate::target::NoteRouter,
    /// The time of the last frame report (the tracker's clock), for the rate of the messages.
    clock: f64,
}

impl std::fmt::Debug for VisionCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisionCore")
            .field("on", &self.session.is_some())
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl Default for VisionCore {
    fn default() -> Self {
        let prefs = CameraPrefs::default();
        Self {
            backend: Arc::new(NoVision),
            session: None,
            preview_on: prefs.preview,
            prefs,
            target: None,
            previous: None,
            pinch: None,
            pad: None,
            pad_now: None,
            firer: GestureFirer::new(),
            commands: Vec::new(),
            hands: BTreeMap::new(),
            seen: BTreeMap::new(),
            dialog: None,
            job: None,
            message: None,
            lamp: CameraLamp::Off,
            fps: 0.0,
            infer_ms: 0.0,
            preview: None,
            midi: CameraMidi::new(),
            notes: crate::target::NoteRouter::new(),
            clock: 0.0,
        }
    }
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ set-up

    /// Installs the camera backend. Only the application window calls it: a core without one
    /// has no camera.
    pub fn set_vision(&mut self, backend: Arc<dyn VisionBackend>) {
        self.camera_off(None);
        self.vision.backend = backend;
    }

    /// The settings of the webcam controller.
    pub fn camera_prefs(&self) -> &CameraPrefs {
        &self.vision.prefs
    }

    /// Applies the settings. A running camera switches to the one chosen.
    pub fn set_camera_prefs(&mut self, prefs: CameraPrefs) {
        if prefs == self.vision.prefs {
            return;
        }
        let camera_changed = prefs.camera != self.vision.prefs.camera;
        let sensitivity_changed = prefs.sensitivity != self.vision.prefs.sensitivity;
        let preview_changed = prefs.preview != self.vision.prefs.preview;
        let mode_changed = prefs.mode != self.vision.prefs.mode
            || prefs.play_notes != self.vision.prefs.play_notes;
        if mode_changed {
            // Notes held under the old settings end before the new ones apply.
            self.camera_midi_release();
        }
        self.vision.prefs = prefs;
        if preview_changed {
            self.vision.preview_on = self.vision.prefs.preview;
            self.sync_camera_preview();
        }
        if sensitivity_changed && self.vision.session.is_some() {
            // The thresholds are the tracker's: start it again with the new ones.
            self.camera_off(None);
            self.camera_on();
        } else if camera_changed
            && let Some(id) = self.vision.prefs.camera.clone()
            && let Some(session) = &mut self.vision.session
        {
            session.switch_camera(&id);
        }
    }

    // ------------------------------------------------------------------ the project's own

    /// The camera settings the project keeps of its own, if it has any.
    pub fn camera_override(&self) -> Option<&CameraOverride> {
        self.doc().camera.as_ref()
    }

    /// Sets or clears the project's own camera settings (one undo step). False when the model
    /// refused them.
    pub fn set_camera_override(&mut self, new: Option<CameraOverride>) -> bool {
        if self.doc().camera == new {
            return true;
        }
        self.apply_command(Command::SetCameraOverride(new)).is_ok()
    }

    /// The camera mode in force: the project's own while it has one, else the settings'.
    pub fn camera_mode(&self) -> CameraMode {
        self.camera_override()
            .and_then(|c| CameraMode::from_id(&c.mode))
            .unwrap_or(self.vision.prefs.mode)
    }

    /// The gesture command table in force: the project's own while it has one, else the
    /// settings'.
    pub fn camera_gesture_table(&self) -> Vec<GestureBinding> {
        match self.camera_override() {
            Some(c) => crate::camera_gestures::from_override(c),
            None => self.vision.prefs.gestures.clone(),
        }
    }

    /// The commands the gestures ran since the last call; the shell runs them like keys.
    pub fn take_camera_commands(&mut self) -> Vec<String> {
        std::mem::take(&mut self.vision.commands)
    }

    /// The cameras the system lists now.
    pub fn camera_list(&self) -> Vec<CameraEntry> {
        self.vision.backend.cameras()
    }

    /// The state of the hand models on this machine.
    pub fn camera_models(&self) -> ModelState {
        self.vision.backend.models()
    }

    /// Deletes the downloaded hand models (the camera goes off first).
    pub fn camera_delete_models(&mut self) {
        self.camera_off(None);
        match self.vision.backend.delete_models() {
            Ok(()) => self.report("The hand models were deleted"),
            Err(e) => self.report(e.to_string()),
        }
    }

    // ------------------------------------------------------------------ the target

    /// The control the camera turns: the last one pressed.
    pub fn camera_target(&self) -> Option<&(DeviceKey, String)> {
        self.vision.target.as_ref()
    }

    /// Makes `(device, param)` the camera target (a press on a control).
    pub(super) fn camera_aim(&mut self, device: &DeviceKey, param: &str) {
        if self
            .vision
            .target
            .as_ref()
            .is_some_and(|(d, p)| d == device && p == param)
        {
            return;
        }
        // A pinch in progress keeps the control it began on; the next pinch uses the new one.
        // The one before it is the pad's other knob.
        self.vision.previous = self
            .vision
            .target
            .replace((device.clone(), param.to_owned()));
        self.vision.message = None;
        self.sync_camera_marks();
    }

    /// A gesture begin of `param` came from `device`'s plugin window, or (`change`) a change
    /// outside any gesture: a click on that control, when a gesture begin gave no other sign
    /// for [`TOUCH_GAP_SECS`].
    pub(super) fn camera_plugin_touch(
        &mut self,
        device: &DeviceKey,
        param: u32,
        change: bool,
        now: f64,
    ) {
        let fresh = self
            .vision
            .seen
            .insert((device.clone(), param), now)
            .is_none_or(|at| !change || now - at >= TOUCH_GAP_SECS);
        if fresh {
            self.camera_aim(device, &format!("p{param}"));
        }
    }

    /// Drops the target when its device is gone (a delete, or an undo of its creation).
    pub(super) fn camera_prune(&mut self) {
        let gone = self
            .vision
            .target
            .as_ref()
            .is_some_and(|(d, p)| self.resolve_param(d, p).is_none());
        let previous_gone = self
            .vision
            .previous
            .as_ref()
            .is_some_and(|(d, p)| self.resolve_param(d, p).is_none());
        if previous_gone {
            self.vision.previous = None;
        }
        if gone {
            self.vision.target = None;
            self.sync_camera_marks();
        }
    }

    /// Hands the views what marks the target (a ring while the camera is on).
    pub(super) fn sync_camera_marks(&mut self) {
        self.sync_midi_marks();
    }

    // ------------------------------------------------------------------ the switch

    /// Whether the camera is on.
    pub fn camera_is_on(&self) -> bool {
        self.vision.session.is_some()
    }

    /// The consent, progress or failure dialog, while one shows.
    pub fn camera_dialog(&self) -> Option<&CameraDialogState> {
        self.vision.dialog.as_ref()
    }

    /// The camera command: off turns it on (asking first when the models are missing), on turns
    /// it off.
    pub fn camera_toggle(&mut self) {
        if self.vision.session.is_some() {
            self.camera_off(None);
        } else {
            self.camera_on();
        }
    }

    /// Turns the camera on. With the models missing or damaged it opens the consent dialog and
    /// fetches nothing.
    pub fn camera_on(&mut self) {
        if self.vision.session.is_some() {
            return;
        }
        self.vision.message = None;
        match self.vision.backend.models() {
            ModelState::Ready => self.camera_start(),
            ModelState::Missing | ModelState::Corrupt => {
                self.vision.dialog = Some(self.consent());
            }
        }
    }

    fn consent(&self) -> CameraDialogState {
        CameraDialogState::Consent {
            source: self.vision.backend.download_source(),
            bytes: self.vision.backend.download_bytes(),
        }
    }

    fn camera_start(&mut self) {
        let config = VisionConfig {
            camera: self.vision.prefs.camera.clone(),
            sensitivity: f32::from(self.vision.prefs.sensitivity) / 100.0,
        };
        match self.vision.backend.start(&config) {
            Ok(mut session) => {
                session.set_preview(self.vision.preview_on);
                self.vision.session = Some(session);
                self.vision.lamp = CameraLamp::NoHand;
                self.vision.hands.clear();
                self.vision.preview = None;
                self.sync_camera_marks();
            }
            Err(e) => {
                let text = e.to_string();
                self.vision.message = Some(text.clone());
                self.report(text);
            }
        }
    }

    /// Turns the camera off: the tracker stops (the camera is released at once), an open pinch
    /// ends and the last picture is forgotten. `why` is said to the user when there is one.
    pub fn camera_off(&mut self, why: Option<&str>) {
        if let Some(mut session) = self.vision.session.take() {
            session.stop();
        }
        self.end_pinch();
        self.end_pad();
        self.camera_midi_release();
        self.vision.midi.reset();
        self.vision.firer.reset();
        self.vision.pad_now = None;
        self.vision.hands.clear();
        self.vision.preview = None;
        self.vision.lamp = CameraLamp::Off;
        self.vision.fps = 0.0;
        self.vision.infer_ms = 0.0;
        self.sync_camera_marks();
        if let Some(why) = why {
            self.vision.message = Some(why.to_owned());
            self.report(why);
        }
    }

    /// A choice in the dialog.
    pub fn camera_dialog_choice(&mut self, choice: atomik_ui::CameraDialogChoice) {
        use atomik_ui::CameraDialogChoice as Choice;
        match (choice, &self.vision.dialog) {
            (Choice::Download | Choice::Retry, Some(CameraDialogState::Consent { .. }))
            | (Choice::Retry, Some(CameraDialogState::Failed { .. })) => self.start_download(),
            (Choice::Cancel, Some(CameraDialogState::Downloading { .. })) => {
                if let Some(job) = &self.vision.job {
                    job.cancel.store(true, Ordering::SeqCst);
                }
            }
            (Choice::Cancel | Choice::Close, Some(_)) => {
                self.vision.job = None;
                self.vision.dialog = None;
            }
            _ => {}
        }
    }

    fn start_download(&mut self) {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(None));
        let result = Arc::new(Mutex::new(None));
        let backend = Arc::clone(&self.vision.backend);
        let (c, p, r) = (
            Arc::clone(&cancel),
            Arc::clone(&progress),
            Arc::clone(&result),
        );
        let worker = std::thread::Builder::new()
            .name("atomik-vision-download".into())
            .spawn(move || {
                let outcome = backend.download(
                    &mut |step| {
                        *p.lock().unwrap_or_else(|e| e.into_inner()) = Some(step);
                    },
                    &c,
                );
                *r.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
            })
            .ok();
        if worker.is_none() {
            self.vision.dialog = Some(CameraDialogState::Failed {
                message: "The download could not be started".into(),
            });
            return;
        }
        self.vision.dialog = Some(CameraDialogState::Downloading {
            received: 0,
            total: self.vision.backend.download_bytes(),
        });
        self.vision.job = Some(Job {
            cancel,
            progress,
            result,
            worker,
        });
    }

    // ------------------------------------------------------------------ the preview

    /// Shows or hides the preview box (`camera.preview`).
    pub fn camera_preview_toggle(&mut self) {
        self.vision.preview_on = !self.vision.preview_on;
        self.sync_camera_preview();
    }

    /// Whether the preview box is switched on (it shows while the camera runs).
    pub fn camera_preview_on(&self) -> bool {
        self.vision.preview_on
    }

    fn sync_camera_preview(&mut self) {
        let wanted = self.vision.preview_on;
        if let Some(session) = &mut self.vision.session {
            session.set_preview(wanted);
        }
        if !wanted {
            self.vision.preview = None;
        }
    }

    /// What the indicator and the preview box show now.
    pub fn camera_preview_state(&self) -> CameraPreviewState {
        let (target, value) = self.camera_target_text();
        let pinched = self
            .vision
            .pinch
            .as_ref()
            .is_some_and(|p| p.merge.is_some())
            || self.vision.pad.as_ref().is_some_and(|p| p.merge.is_some());
        CameraPreviewState {
            lamp: self.vision.lamp,
            picture: self
                .vision
                .preview
                .as_ref()
                .map(|p| atomik_ui::CameraPicture {
                    width: p.width,
                    height: p.height,
                    rgb: p.rgb.clone(),
                }),
            hands: self
                .vision
                .preview
                .as_ref()
                .map(|p| {
                    p.hands
                        .iter()
                        .map(|h| h.landmarks.iter().map(|l| (l.x, l.y)).collect())
                        .collect()
                })
                .unwrap_or_default(),
            target: target.filter(|_| pinched),
            value: value.filter(|_| pinched),
            message: self.vision.message.clone(),
            fps: self.vision.fps,
            infer_ms: self.vision.infer_ms,
            controls: self.camera_controls_state(),
            pad: self
                .vision
                .pad_now
                .filter(|_| self.vision.session.is_some())
                // The preview is mirrored: moving to the right of it is a smaller x.
                .map(|p| (1.0 - p.x, p.y, p.spread)),
        }
    }

    /// The target's name and its value as text.
    pub fn camera_target_text(&self) -> (Option<String>, Option<String>) {
        let Some((device, param)) = &self.vision.target else {
            return (None, None);
        };
        let Some(Resolved::Param { key, desc }) = self.resolve_param(device, param) else {
            return (None, None);
        };
        let value = self
            .doc()
            .param_value(&key, desc.key, &self.registry)
            .map(|v| atomik_ui::widget::format_value(desc, v));
        (
            Some(format!("{} {}", self.device_label(&key), desc.name)),
            value,
        )
    }

    // ------------------------------------------------------------------ the frame

    /// Once a frame: the download's progress and answer, the tracker's events and the preview
    /// picture.
    pub(super) fn camera_frame(&mut self) {
        self.camera_prune();
        self.poll_download();
        let Some(session) = &mut self.vision.session else {
            return;
        };
        let events = session.poll();
        let preview = session.preview();
        let alive = session.running();
        if let Some(p) = preview {
            self.vision.preview = Some(p);
        }
        for event in events {
            if self.vision.session.is_none() {
                break;
            }
            self.camera_event(event);
        }
        if self.vision.session.is_some() && !alive {
            self.camera_off(Some("The camera stopped"));
        }
    }

    fn poll_download(&mut self) {
        let Some(job) = &self.vision.job else { return };
        let answer = job.result.lock().unwrap_or_else(|e| e.into_inner()).take();
        let progress = *job.progress.lock().unwrap_or_else(|e| e.into_inner());
        match answer {
            None => {
                if let (Some(p), Some(CameraDialogState::Downloading { received, total })) =
                    (progress, &mut self.vision.dialog)
                {
                    *received = p.received;
                    *total = p.total;
                }
            }
            Some(Ok(())) => {
                self.vision.job = None;
                self.vision.dialog = None;
                self.camera_start();
            }
            Some(Err(e)) => {
                let cancelled = job.cancel.load(Ordering::SeqCst);
                self.vision.job = None;
                self.vision.dialog = if cancelled {
                    Some(self.consent())
                } else {
                    Some(CameraDialogState::Failed {
                        message: e.to_string(),
                    })
                };
            }
        }
    }

    fn camera_event(&mut self, event: TrackerEvent) {
        match event {
            TrackerEvent::Started { .. } => {}
            TrackerEvent::Gesture(g) => self.camera_gesture(g),
            TrackerEvent::Frame(report) => self.camera_report(&report),
            TrackerEvent::Failed(message) => {
                self.camera_off(Some(&format!("The camera stopped: {message}")));
            }
            TrackerEvent::Stopped => self.camera_off(Some("The camera stopped")),
        }
    }

    fn camera_report(&mut self, report: &FrameReport) {
        self.vision.fps = report.fps;
        self.vision.infer_ms = report.infer_ms;
        self.vision.clock = report.t;
        self.vision.hands = report.hands.iter().map(|h| (h.id, *h)).collect();
        self.vision.pad_now = report.pad;
        let any = report.hands.iter().any(|h| h.present);
        let flags = self.camera_flags();
        let midi_pinch = flags.midi && report.hands.iter().any(|h| h.present && h.pinch);
        self.vision.lamp = if self.vision.pinch.is_some() || self.vision.pad.is_some() || midi_pinch
        {
            CameraLamp::Pinch
        } else if any {
            CameraLamp::Hand
        } else {
            CameraLamp::NoHand
        };
        // A pinch that was waiting for the hand's size has it now.
        let waiting = self
            .vision
            .pinch
            .as_ref()
            .filter(|p| p.scale.is_none())
            .map(|p| p.hand);
        if let Some(hand) = waiting
            && let Some(state) = self.vision.hands.get(&hand).copied()
        {
            let scale = fine_scale(self.vision.prefs.fine, state.depth_proxy);
            let pending = if let Some(p) = &mut self.vision.pinch {
                p.scale = Some(scale);
                std::mem::take(&mut p.pending)
            } else {
                0.0
            };
            if pending != 0.0 {
                self.twist(pending);
            }
        }
        self.pad_move(report.pad);
        self.camera_midi_frame(report.t, &report.hands, report.pad);
        self.camera_poll_commands();
    }

    // ------------------------------------------------------------------ the gesture

    fn camera_gesture(&mut self, event: GestureEvent) {
        let flags = self.camera_flags();
        if flags.midi {
            self.camera_midi_gesture(&event, flags.twist);
        }
        self.camera_gesture_command(&event);
        match event {
            GestureEvent::PinchStart { hand, handedness } => {
                if !flags.turns
                    || !self.vision.prefs.hand.allows(handedness.letter())
                    || self.vision.pinch.is_some()
                    || self.vision.pad.is_some()
                {
                    return;
                }
                self.begin_pinch(hand);
            }
            GestureEvent::PadStart => {
                if flags.turns {
                    // Two hands take over from the one that pinched first.
                    self.end_pinch();
                    self.begin_pad();
                }
            }
            GestureEvent::PadEnd => self.end_pad(),
            GestureEvent::Twist {
                hand,
                delta_radians,
            } => {
                if self.vision.pinch.as_ref().is_some_and(|p| p.hand == hand) {
                    self.twist(delta_radians);
                }
            }
            GestureEvent::PinchEnd { hand } => {
                if self.vision.pinch.as_ref().is_some_and(|p| p.hand == hand) {
                    self.end_pinch();
                }
            }
            GestureEvent::OpenPalmStart { .. }
            | GestureEvent::OpenPalmEnd { .. }
            | GestureEvent::PoseStart { .. }
            | GestureEvent::PoseEnd { .. }
            | GestureEvent::Swipe { .. } => {}
        }
    }

    /// Tells the firer what the hands did and queues the commands that are due.
    fn camera_gesture_command(&mut self, event: &GestureEvent) {
        let t = self.vision.clock;
        match *event {
            GestureEvent::PoseStart {
                hand,
                handedness,
                pose,
            } => self
                .vision
                .firer
                .start(t, hand, handedness, gesture_of(pose)),
            GestureEvent::OpenPalmStart { hand, handedness } => {
                self.vision
                    .firer
                    .start(t, hand, handedness, CameraGesture::OpenPalm);
            }
            GestureEvent::PoseEnd { hand, pose } => {
                self.vision.firer.end(hand, gesture_of(pose));
            }
            GestureEvent::OpenPalmEnd { hand } => {
                self.vision.firer.end(hand, CameraGesture::OpenPalm);
            }
            GestureEvent::Swipe {
                handedness,
                direction,
                ..
            } => {
                let table = self.camera_gesture_table();
                let pinching = self.camera_pinching();
                if let Some(command) = self.vision.firer.swipe(
                    t,
                    handedness,
                    swipe_gesture(direction),
                    &table,
                    pinching,
                ) {
                    self.vision.commands.push(command);
                }
            }
            _ => return,
        }
        self.camera_poll_commands();
    }

    /// Queues the commands of the holds that are ready.
    fn camera_poll_commands(&mut self) {
        let table = self.camera_gesture_table();
        let pinching = self.camera_pinching();
        let t = self.vision.clock;
        let ready = self.vision.firer.poll(t, &table, pinching);
        self.vision.commands.extend(ready);
    }

    /// Whether any hand is pinched (a knob, or the pad, is being turned).
    fn camera_pinching(&self) -> bool {
        self.vision.pinch.is_some()
            || self.vision.pad.is_some()
            || self.vision.hands.values().any(|h| h.present && h.pinch)
    }

    // ------------------------------------------------------------------ the pad

    /// Both hands pinched: the last two clicked knobs are the pad's X and Y.
    fn begin_pad(&mut self) {
        let pair = match (&self.vision.previous, &self.vision.target) {
            (Some(x), Some(y)) => Some([x.clone(), y.clone()]),
            _ => None,
        };
        let starts = pair.as_ref().and_then(|[x, y]| {
            let value = |(d, p): &(DeviceKey, String)| match self.resolve_param(d, p) {
                Some(Resolved::Param { key, desc }) => {
                    let plain = self
                        .doc()
                        .param_value(&key, desc.key, &self.registry)
                        .unwrap_or(desc.default);
                    Some(desc.to_normalized(plain))
                }
                _ => None,
            };
            Some([value(x)?, value(y)?])
        });
        self.vision.lamp = CameraLamp::Pinch;
        let Some((targets, start)) = pair.zip(starts) else {
            self.vision.message = Some(NO_PAD_TARGETS_MESSAGE.to_owned());
            self.vision.pad = Some(PadDrag {
                merge: None,
                targets: None,
                start: [0.0; 2],
                origin: None,
            });
            self.report(NO_PAD_TARGETS_MESSAGE);
            return;
        };
        let merge = self.midi.gesture(GestureKey::Camera, true);
        self.vision.message = None;
        self.vision.pad = Some(PadDrag {
            merge: Some(merge),
            targets: Some(targets),
            start,
            origin: None,
        });
    }

    /// The hands moved: both knobs follow, in one batch.
    fn pad_move(&mut self, pad: Option<Pad>) {
        let Some(pad) = pad else { return };
        let Some(drag) = &mut self.vision.pad else {
            return;
        };
        let (Some(merge), Some(targets)) = (drag.merge, drag.targets.clone()) else {
            return;
        };
        let origin = *drag.origin.get_or_insert((pad.x, pad.y));
        // To the right of the mirrored preview is a smaller x; up is a smaller y.
        let moved = [
            (origin.0 - pad.x) / PAD_RANGE,
            (origin.1 - pad.y) / PAD_RANGE,
        ];
        let start = drag.start;
        let mut commands = Vec::new();
        let mut changed = false;
        for (i, (device, param)) in targets.iter().enumerate() {
            let Some(Resolved::Param { key, desc }) = self.resolve_param(device, param) else {
                // A knob went away under the hands.
                self.end_pad();
                return;
            };
            let plain = desc.from_normalized((start[i] + moved[i]).clamp(0.0, 1.0));
            let old = self.doc().param_value(&key, desc.key, &self.registry);
            changed |= old.is_none_or(|o| o.to_bits() != plain.to_bits());
            commands.push(Command::SetParam {
                key,
                param: desc.key.to_owned(),
                value: plain,
                merge: Some(merge),
            });
        }
        if changed {
            // A rejection (a macro-driven knob, a frozen track) is shown like any other.
            let _ = self.apply_command(Command::Batch(commands));
        }
    }

    /// Lets go of the pad: the gesture closes, so both moves are one undo step.
    fn end_pad(&mut self) {
        let Some(pad) = self.vision.pad.take() else {
            return;
        };
        if pad.merge.is_some() && self.midi.close_gesture(&GestureKey::Camera) {
            self.document.end_merge();
            self.release_automation();
        }
        if self.vision.lamp == CameraLamp::Pinch && self.vision.pinch.is_none() {
            self.vision.lamp = if self.vision.session.is_some() {
                CameraLamp::Hand
            } else {
                CameraLamp::Off
            };
        }
        if pad.merge.is_some() {
            self.vision.message = None;
        }
    }

    fn begin_pinch(&mut self, hand: HandId) {
        let target = self.vision.target.clone();
        let resolved = target.as_ref().and_then(|(d, p)| self.resolve_param(d, p));
        let Some(Resolved::Param { key, desc }) = resolved else {
            // Nothing to turn: say so, once per pinch.
            self.vision.message = Some(NO_TARGET_MESSAGE.to_owned());
            self.vision.lamp = CameraLamp::Pinch;
            self.vision.pinch = Some(Pinch {
                hand,
                merge: None,
                start: 0.0,
                start_plain: 0.0,
                moved: 0.0,
                step: 0,
                scale: Some(1.0),
                pending: 0.0,
            });
            self.report(NO_TARGET_MESSAGE);
            return;
        };
        let plain = self
            .doc()
            .param_value(&key, desc.key, &self.registry)
            .unwrap_or(desc.default);
        let merge = self.midi.gesture(GestureKey::Camera, true);
        let scale = self
            .vision
            .hands
            .get(&hand)
            .map(|h| fine_scale(self.vision.prefs.fine, h.depth_proxy));
        self.vision.message = None;
        self.vision.lamp = CameraLamp::Pinch;
        self.vision.pinch = Some(Pinch {
            hand,
            merge: Some(merge),
            start: desc.to_normalized(plain),
            start_plain: plain,
            moved: 0.0,
            step: 0,
            scale,
            pending: 0.0,
        });
    }

    /// A twist of `radians` (positive: clockwise) turns the target.
    fn twist(&mut self, radians: f32) {
        let full_turn = f32::from(self.vision.prefs.full_turn).clamp(90.0, 360.0);
        let Some(pinch) = &mut self.vision.pinch else {
            return;
        };
        let Some(scale) = pinch.scale else {
            pinch.pending += radians;
            return;
        };
        if pinch.merge.is_none() {
            return;
        }
        let delta = radians.to_degrees() / full_turn * scale;
        let (start, start_plain) = (pinch.start, pinch.start_plain);
        pinch.moved = (pinch.moved + delta).clamp(-start, 1.0 - start);
        let (moved, merge) = (pinch.moved, pinch.merge);
        let Some((device, param)) = self.vision.target.clone() else {
            return;
        };
        let Some(Resolved::Param { key, desc }) = self.resolve_param(&device, &param) else {
            // The control went away under the hand.
            self.end_pinch();
            self.vision.target = None;
            self.sync_camera_marks();
            return;
        };
        let norm = if desc.discrete {
            // One step for each step width of turn counted from where the pinch began. A step
            // is only taken back after a whole step width the other way, so a hand trembling at
            // a boundary does not make the value flicker.
            let turned = moved * (desc.max - desc.min);
            let mut step = self.vision.pinch.as_ref().map_or(0, |p| p.step);
            while turned >= (step + 1) as f32 {
                step += 1;
            }
            while turned <= (step - 1) as f32 {
                step -= 1;
            }
            if let Some(p) = &mut self.vision.pinch {
                p.step = step;
            }
            desc.to_normalized(desc.clamp(start_plain + step as f32))
        } else {
            start + moved
        };
        self.set_resolved(&Resolved::Param { key, desc }, norm, merge);
    }

    /// Lets go: the gesture closes, so the move is one undo step and a Touch take stops writing.
    fn end_pinch(&mut self) {
        let Some(pinch) = self.vision.pinch.take() else {
            return;
        };
        if pinch.merge.is_some() && self.midi.close_gesture(&GestureKey::Camera) {
            self.document.end_merge();
            self.release_automation();
        }
        if self.vision.lamp == CameraLamp::Pinch {
            self.vision.lamp = if self.vision.session.is_some() {
                CameraLamp::Hand
            } else {
                CameraLamp::Off
            };
        }
        if pinch.merge.is_some() {
            self.vision.message = None;
        }
    }
}

#[path = "app_core_camera_midi.rs"]
mod camera_midi_ops;

#[cfg(test)]
#[path = "app_core_vision_tests.rs"]
mod tests;
