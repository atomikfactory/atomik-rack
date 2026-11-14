//! Meter panels on console strips and plugin devices.
//!
//! A **spectrum** or **phase** panel opens for one console strip (a channel, a bus, the master)
//! or one plugin device from its right-click menu, without inserting a device
//! ([`AppCore::open_meter_panel`]); several can be open at once and a panel closes when the
//! strip or device it looks at is deleted. The panels are view state: they are not saved.
//!
//! # The tap
//!
//! A strip and a plugin device each own a [`atomik_device_sdk::ScopeTap`], the ring the spectrum
//! and phase meter devices use. While a panel shows the strip, the app sets the live flag of the
//! tap ([`EngineSink::set_device_scope_live`], sent again every frame so that a rebuilt engine or
//! a replaced device is picked up) and the device writes its output into the ring after its
//! fader (a plugin device: after its bypass mix). With no panel open the flag is off and the
//! audio thread only reads it once per batch. The pre-fader switch of the Spectrum EQ window wins
//! when it looks at the same strip.
//!
//! # The analysis
//!
//! The FFT is the one the panel of the spectrum device runs ([`atomik_dsp::spectrum`]), done here
//! once a frame on the newest frames of the tap and never on the audio thread; a tap that did not
//! move since the last frame (a stopped engine) is skipped. The ring of a strip holds 32768
//! frames, so its panel offers sizes up to 16384; the spectrum *device* has a larger ring and
//! reaches 65536. The phase panel keeps the newest 2048 frames for the goniometer and measures
//! their correlation and the level of each side.
//!
//! # The second curve
//!
//! The spectrum panel can draw a second source over its trace: another strip (read through its
//! own tap, which is then switched on too) or the reference track (its long-term average
//! spectrum, measured with the file and moved by the matched gain and the trim, so it shows at
//! the level it is heard at).

use super::*;
use atomik_device_sdk::SCOPE_FRAMES;
use atomik_devices::spectrum::SIZES;
use atomik_dsp::spectrum::{Settings, Spectrum};
use atomik_ui::analyser::{SCOPE_READ_FRAMES, TRACE_COLUMNS};
use atomik_ui::{CurveSource, MeterKind, MeterPanelAction, MeterPanelState};
use std::collections::BTreeSet;

/// The FFT sizes a strip panel offers: up to half its ring.
fn strip_sizes() -> &'static [usize] {
    let n = SIZES.iter().take_while(|s| **s <= SCOPE_FRAMES / 2).count();
    &SIZES[..n]
}

/// The size a new panel starts at.
const DEFAULT_SIZE: usize = 4096;

/// Where the second curve of a panel comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurveFrom {
    /// No second curve.
    Off,
    /// Another strip.
    Strip(DeviceKey),
    /// The reference track of the song.
    Reference,
}

/// One open panel.
struct Panel {
    id: u32,
    target: DeviceKey,
    kind: MeterKind,
    size: usize,
    from: CurveFrom,
    spectrum: Spectrum,
    second: Spectrum,
    head: u64,
    second_head: u64,
    left: Vec<f32>,
    right: Vec<f32>,
    /// The frames of the second strip.
    left2: Vec<f32>,
    right2: Vec<f32>,
    scope_l: Vec<f32>,
    scope_r: Vec<f32>,
    correlation: f32,
    peaks: [f32; 2],
}

/// The meter panels of an open project (not saved).
#[derive(Default)]
pub(super) struct MeterPanelsState {
    panels: Vec<Panel>,
    next_id: u32,
    /// The engine devices whose tap was told to be live.
    live: BTreeSet<DeviceInstanceId>,
}

impl MeterPanelsState {
    /// A new engine: every tap is new and off.
    pub(super) fn restart(&mut self) {
        self.live.clear();
        for p in &mut self.panels {
            p.head = 0;
            p.second_head = 0;
        }
    }

    /// Forgets every panel (another project).
    pub(super) fn clear(&mut self) {
        self.panels.clear();
        self.live.clear();
    }
}

fn db(x: f32) -> f32 {
    if x <= 1e-6 { -120.0 } else { 20.0 * x.log10() }
}

impl<S: EngineSink> AppCore<S> {
    /// Whether a meter can open on `key`: a console strip or a plugin device.
    pub fn can_open_meter(&self, key: &DeviceKey) -> bool {
        self.console.strip_names().iter().any(|(k, _, _)| k == key)
            || self.plugin_instance(key).is_some()
    }

    /// Opens a meter panel of `kind` on `target` and returns its id; a panel of that kind that is
    /// open for the target is returned as it is.
    ///
    /// # Errors
    /// The device is neither a console strip nor a plugin device (the reason is also in the
    /// status line).
    pub fn open_meter_panel(&mut self, target: &DeviceKey, kind: MeterKind) -> Result<u32, String> {
        if let Some(p) = self
            .meter_panels
            .panels
            .iter()
            .find(|p| &p.target == target && p.kind == kind)
        {
            return Ok(p.id);
        }
        if self.doc().device(target).is_none() || !self.can_open_meter(target) {
            let why = "A meter opens on a console strip or a plugin device".to_owned();
            self.reject(why.clone());
            return Err(why);
        }
        self.meter_panels.next_id += 1;
        let id = self.meter_panels.next_id;
        self.meter_panels.panels.push(Panel {
            id,
            target: target.clone(),
            kind,
            size: DEFAULT_SIZE,
            from: CurveFrom::Off,
            spectrum: Spectrum::new(),
            second: Spectrum::new(),
            head: 0,
            second_head: 0,
            left: Vec::new(),
            right: Vec::new(),
            left2: Vec::new(),
            right2: Vec::new(),
            scope_l: Vec::new(),
            scope_r: Vec::new(),
            correlation: 0.0,
            peaks: [-120.0; 2],
        });
        self.feed_meter_panels();
        Ok(id)
    }

    /// Closes panel `id`.
    pub fn close_meter_panel(&mut self, id: u32) {
        self.meter_panels.panels.retain(|p| p.id != id);
        self.feed_meter_panels();
    }

    /// The ids of the open panels, oldest first.
    pub fn meter_panel_ids(&self) -> Vec<u32> {
        self.meter_panels.panels.iter().map(|p| p.id).collect()
    }

    /// The strip or device panel `id` looks at.
    pub fn meter_panel_target(&self, id: u32) -> Option<&DeviceKey> {
        self.meter_panels
            .panels
            .iter()
            .find(|p| p.id == id)
            .map(|p| &p.target)
    }

    /// The FFT size of panel `id`.
    pub fn meter_panel_size(&self, id: u32) -> Option<usize> {
        self.meter_panels
            .panels
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.size)
    }

    /// The second curve of panel `id`.
    pub fn meter_panel_curve(&self, id: u32) -> Option<CurveFrom> {
        self.meter_panels
            .panels
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.from.clone())
    }

    /// The choices of the second curve of a panel on `target`: no curve, the other strips in
    /// console order, then the reference track when the song has one. The name is the strip's.
    pub fn meter_curve_choices(&self, target: &DeviceKey) -> Vec<(CurveFrom, String)> {
        let mut out = vec![(CurveFrom::Off, String::new())];
        for (key, name, _) in self.console.strip_names() {
            if &key != target {
                out.push((CurveFrom::Strip(key), name));
            }
        }
        if !self.doc().references.is_empty() {
            out.push((CurveFrom::Reference, String::new()));
        }
        out
    }

    /// Picks the second curve of panel `id`.
    pub fn set_meter_panel_curve(&mut self, id: u32, from: CurveFrom) {
        let Some(at) = self.meter_panels.panels.iter().position(|p| p.id == id) else {
            return;
        };
        let valid = self
            .meter_curve_choices(&self.meter_panels.panels[at].target)
            .iter()
            .any(|(c, _)| *c == from);
        if !valid {
            return;
        }
        let p = &mut self.meter_panels.panels[at];
        p.from = from;
        p.second = Spectrum::new();
        p.second_head = 0;
        self.feed_meter_panels();
    }

    /// Steps the FFT size of panel `id` one place; the ring of a strip bounds it.
    pub fn step_meter_panel_size(&mut self, id: u32, up: bool) {
        let sizes = strip_sizes();
        let Some(p) = self.meter_panels.panels.iter_mut().find(|p| p.id == id) else {
            return;
        };
        let at = sizes.iter().position(|s| *s == p.size).unwrap_or(2);
        let next = if up {
            (at + 1).min(sizes.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        p.size = sizes[next];
        p.head = 0;
        p.second_head = 0;
        self.feed_meter_panels();
    }

    /// What a floating panel asked for.
    pub fn meter_panel_action(&mut self, action: MeterPanelAction) {
        match action {
            MeterPanelAction::Close(id) => self.close_meter_panel(id),
            MeterPanelAction::Size { id, up } => self.step_meter_panel_size(id, up),
            MeterPanelAction::Source { id, index } => {
                let Some(target) = self.meter_panel_target(id).cloned() else {
                    return;
                };
                if let Some((from, _)) = self.meter_curve_choices(&target).into_iter().nth(index) {
                    self.set_meter_panel_curve(id, from);
                }
            }
        }
    }

    /// What the view of the panels shows, in opening order.
    pub fn meter_panel_states(&self) -> Vec<MeterPanelState> {
        let shift = self.reference_curve_shift();
        self.meter_panels
            .panels
            .iter()
            .map(|p| {
                let choices = self.meter_curve_choices(&p.target);
                let source = choices.iter().position(|(c, _)| *c == p.from).unwrap_or(0);
                let sources = choices
                    .iter()
                    .map(|(c, name)| match c {
                        CurveFrom::Off => CurveSource::Off,
                        CurveFrom::Strip(_) => CurveSource::Strip(name.clone()),
                        CurveFrom::Reference => CurveSource::Reference,
                    })
                    .collect();
                let second = match &p.from {
                    CurveFrom::Off => Vec::new(),
                    CurveFrom::Strip(_) => p.second.trace().to_vec(),
                    CurveFrom::Reference => self
                        .reference_trace()
                        .map(|t| t.iter().map(|v| v + shift).collect())
                        .unwrap_or_default(),
                };
                MeterPanelState {
                    id: p.id,
                    kind: p.kind,
                    name: self.meter_name(&p.target),
                    trace: p.spectrum.trace().to_vec(),
                    peak_hz: p.spectrum.peak_hz(),
                    peak_db: p.spectrum.peak_db(),
                    size: p.size,
                    second,
                    sources,
                    source,
                    scope_l: p.scope_l.clone(),
                    scope_r: p.scope_r.clone(),
                    correlation: p.correlation,
                    left_db: p.peaks[0],
                    right_db: p.peaks[1],
                }
            })
            .collect()
    }

    /// The name a panel shows for `key`: the name of the strip, else of the device.
    fn meter_name(&self, key: &DeviceKey) -> String {
        self.console
            .strip_names()
            .into_iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, name, _)| name)
            .or_else(|| self.doc().device(key).map(|d| d.name.clone()))
            .unwrap_or_default()
    }

    /// Closes every panel (another project was installed).
    pub(super) fn meter_panels_clear(&mut self) {
        self.meter_panels.clear();
    }

    /// Whether the tap of the strip or device `key` is switched on for a panel.
    pub fn meter_tap_live(&self, key: &DeviceKey) -> bool {
        self.ids
            .get(key)
            .is_some_and(|id| self.meter_panels.live.contains(id))
    }

    /// Feeds the panels (module docs); once per frame from [`Self::frame`].
    pub(super) fn feed_meter_panels(&mut self) {
        if self.meter_panels.panels.is_empty() && self.meter_panels.live.is_empty() {
            return;
        }
        // A strip or device that is gone closes its panels; a second curve that is gone clears.
        let gone: Vec<u32> = self
            .meter_panels
            .panels
            .iter()
            .filter(|p| self.doc().device(&p.target).is_none())
            .map(|p| p.id)
            .collect();
        if !gone.is_empty() {
            self.meter_panels.panels.retain(|p| !gone.contains(&p.id));
        }
        let strips: Vec<DeviceKey> = self
            .console
            .strip_names()
            .into_iter()
            .map(|(k, _, _)| k)
            .collect();
        for p in &mut self.meter_panels.panels {
            if let CurveFrom::Strip(k) = &p.from
                && !strips.contains(k)
            {
                p.from = CurveFrom::Off;
                p.second = Spectrum::new();
                p.second_head = 0;
            }
        }
        // The taps that should write: every panel target and every strip drawn as a curve.
        let mut wanted: BTreeSet<DeviceInstanceId> = BTreeSet::new();
        for p in &self.meter_panels.panels {
            if let Some(id) = self.ids.get(&p.target) {
                wanted.insert(*id);
            }
            if let CurveFrom::Strip(k) = &p.from
                && let Some(id) = self.ids.get(k)
            {
                wanted.insert(*id);
            }
        }
        for id in self.meter_panels.live.clone() {
            if !wanted.contains(&id) {
                // The device may be gone: a refusal is nothing to show.
                self.sink.set_device_scope_live(id, false);
                self.meter_panels.live.remove(&id);
            }
        }
        for id in &wanted {
            // Sent every frame: a replaced device has a new tap that starts off.
            self.sink.set_device_scope_live(*id, true);
            self.meter_panels.live.insert(*id);
        }
        let sample_rate = self.doc().sample_rate as f32;
        let mut state = std::mem::take(&mut self.meter_panels);
        for p in &mut state.panels {
            let Some(id) = self.ids.get(&p.target).copied() else {
                continue;
            };
            let settings = Settings {
                size: p.size,
                averaging_ms: 120.0,
                hold_ms: 0.0,
                ..Settings::default()
            };
            match p.kind {
                MeterKind::Spectrum => {
                    p.left.resize(p.size, 0.0);
                    p.right.resize(p.size, 0.0);
                    if let Some((n, head)) =
                        self.sink.read_device_scope(id, &mut p.left, &mut p.right)
                        && head != 0
                        && head != p.head
                    {
                        let elapsed = head.saturating_sub(p.head);
                        p.head = head;
                        p.spectrum.update(
                            &p.left[..n],
                            &p.right[..n],
                            elapsed,
                            sample_rate,
                            &settings,
                            TRACE_COLUMNS,
                        );
                    }
                    if let CurveFrom::Strip(key) = p.from.clone()
                        && let Some(other) = self.ids.get(&key).copied()
                    {
                        p.left2.resize(p.size, 0.0);
                        p.right2.resize(p.size, 0.0);
                        if let Some((n, head)) =
                            self.sink
                                .read_device_scope(other, &mut p.left2, &mut p.right2)
                            && head != 0
                            && head != p.second_head
                        {
                            let elapsed = head.saturating_sub(p.second_head);
                            p.second_head = head;
                            p.second.update(
                                &p.left2[..n],
                                &p.right2[..n],
                                elapsed,
                                sample_rate,
                                &settings,
                                TRACE_COLUMNS,
                            );
                        }
                    }
                }
                MeterKind::Phase => {
                    p.left.resize(SCOPE_READ_FRAMES, 0.0);
                    p.right.resize(SCOPE_READ_FRAMES, 0.0);
                    if let Some((n, head)) =
                        self.sink.read_device_scope(id, &mut p.left, &mut p.right)
                        && head != 0
                        && head != p.head
                    {
                        p.head = head;
                        let (l, r) = (&p.left[..n], &p.right[..n]);
                        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
                        let (mut pl, mut pr) = (0.0f32, 0.0f32);
                        for (a, b) in l.iter().zip(r) {
                            ll += f64::from(a * a);
                            rr += f64::from(b * b);
                            lr += f64::from(a * b);
                            pl = pl.max(a.abs());
                            pr = pr.max(b.abs());
                        }
                        let denom = (ll * rr).sqrt();
                        p.correlation = if denom > 1e-12 {
                            (lr / denom).clamp(-1.0, 1.0) as f32
                        } else {
                            0.0
                        };
                        p.peaks = [db(pl), db(pr)];
                        p.scope_l = l.iter().step_by(2).copied().collect();
                        p.scope_r = r.iter().step_by(2).copied().collect();
                    }
                }
            }
        }
        self.meter_panels = state;
    }
}
