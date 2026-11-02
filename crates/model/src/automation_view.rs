//! What a control shows for an automated param.
//!
//! The document holds one value per param; a song that plays an automation lane drives the
//! param to the lane's value at every batch without touching the document. A control therefore
//! shows an **overlay**: the lane's value at the transport position while a lane plays the
//! param, the document's value otherwise. This module computes that overlay from the document
//! alone ([`RackDoc::automation_values_at`]), so it needs no help from the engine and works
//! with a stopped transport. It follows the song the engine is given (`atomik_render`'s
//! `song_input`) lane by lane: the tracks that play (mute and solo applied), a frozen track
//! playing its render instead, a lane with no clips playing nothing, a param a macro bank drives
//! following the macro, and the lane value rule of the player ([`automation_value_at`]).
//!
//! The overlay is a copy for the views ([`RackDoc::view_with_shown`]); the document, its undo
//! history and its dirty state never see it.

use crate::song::{effective_tracks, flatten_automation};
use crate::{Curve, DeviceKey, LaneKind, RackDoc};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Values some controls show instead of the stored ones, by `(device, param key)`.
pub type ShownValues = BTreeMap<(DeviceKey, String), f32>;

/// The value of flattened automation `points` (`(song tick, value, curve)`, strictly increasing
/// ticks, see [`flatten_automation`]) at the fractional song tick `tick`: held before the first
/// and after the last point, linear or step in between. This is the rule the song player applies
/// to a lane at every batch; no points give 0.
pub fn automation_value_at(points: &[(i64, f32, Curve)], tick: f64) -> f32 {
    let k = points.partition_point(|p| p.0 as f64 <= tick);
    let Some(&(t0, v0, curve)) = k.checked_sub(1).and_then(|i| points.get(i)) else {
        return points.first().map_or(0.0, |p| p.1);
    };
    match (curve, points.get(k)) {
        (Curve::Linear, Some(&(t1, v1, _))) => {
            let x = (tick - t0 as f64) / (t1 - t0) as f64;
            (f64::from(v0) + (f64::from(v1) - f64::from(v0)) * x) as f32
        }
        _ => v0,
    }
}

/// The flattened points of the playing automation lanes of a document, by `(device, param key)`
/// in song order (see [`RackDoc::automation_curves`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutomationCurves(Vec<(LaneKey, Vec<FlatPoint>)>);

type LaneKey = (DeviceKey, String);
type FlatPoint = (i64, f32, Curve);

impl AutomationCurves {
    /// True when no lane plays.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What each lane gives its param at the fractional song tick `tick`.
    pub fn values_at(&self, tick: f64) -> ShownValues {
        let mut out = ShownValues::new();
        for (key, points) in &self.0 {
            out.insert(key.clone(), automation_value_at(points, tick));
        }
        out
    }
}

impl RackDoc {
    /// The params that have an automation lane, by `(device, param key)`, whether or not the
    /// lane has clips or its track plays. What a control marks as automated.
    pub fn automated_params(&self) -> BTreeSet<(DeviceKey, String)> {
        let mut out = BTreeSet::new();
        for track in &self.song.tracks {
            for lane in &track.lanes {
                if let LaneKind::Automation(key) = &lane.kind {
                    out.insert((track.device.clone(), key.clone()));
                }
            }
        }
        out
    }

    /// The value each playing automation lane gives its param at the fractional song tick
    /// `tick`, by `(device, param key)`: what the song player applies there. A lane without
    /// clips, one of a muted track (or of an unsoloed one while another is soloed) or of a frozen
    /// track, and one on a param a macro bank drives, gives nothing.
    pub fn automation_values_at(&self, tick: f64) -> ShownValues {
        self.automation_curves().values_at(tick)
    }

    /// The flattened points of every playing automation lane (see
    /// [`Self::automation_values_at`] for which play). They depend on the document alone, so a
    /// caller that asks for many positions flattens once per document revision and evaluates
    /// with [`AutomationCurves::values_at`].
    pub fn automation_curves(&self) -> AutomationCurves {
        let mut out = Vec::new();
        for track in effective_tracks(&self.song) {
            if track.frozen.is_some() {
                continue;
            }
            for (i, lane) in track.lanes.iter().enumerate() {
                let LaneKind::Automation(key) = &lane.kind else {
                    continue;
                };
                if self.bank_driver(&track.device, key).is_some() {
                    continue;
                }
                let points = flatten_automation(track, i);
                if points.is_empty() {
                    continue;
                }
                out.push(((track.device.clone(), key.clone()), points));
            }
        }
        AutomationCurves(out)
    }

    /// The document as the views show it: `shown` replaces the stored value of each param it
    /// names (a name that matches no device is ignored). The copy has an empty song (no view
    /// reads it, and it is the heavy part); with nothing to show it is the document itself.
    pub fn view_with_shown(&self, shown: &ShownValues) -> Cow<'_, RackDoc> {
        if shown.is_empty() {
            return Cow::Borrowed(self);
        }
        let mut devices = self.devices.clone();
        for ((device, param), value) in shown {
            if let Some(d) = devices.iter_mut().find(|d| &d.key == device) {
                d.params.insert(param.clone(), *value);
            }
        }
        Cow::Owned(RackDoc {
            sample_rate: self.sample_rate,
            tempo_bpm: self.tempo_bpm,
            seed: self.seed,
            output_channels: self.output_channels,
            input_channels: self.input_channels,
            assets: self.assets.clone(),
            devices,
            columns: self.columns,
            cables: self.cables.clone(),
            song: Arc::new(crate::empty_song(self.tempo_bpm)),
            console: self.console.clone(),
            midi_maps: Vec::new(),
            macros: self.macros.clone(),
            info: self.info.clone(),
            tune_cents: self.tune_cents,
        })
    }
}
