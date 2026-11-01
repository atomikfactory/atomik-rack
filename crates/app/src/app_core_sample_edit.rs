//! Sample edits as render operations: crop, normalise, reverse and fades of a zone's sample.
//!
//! An asset never changes (it is named by its content), so an edit **renders a new asset**: the
//! worker `atomik-sample-edit` ([`crate::audio_import::spawn_sample_edit`]) reads the old audio,
//! applies one of the pure functions of `atomik_audio_file::edit` to a frame range and stores the
//! result in the project bundle. When it answers, [`AppCore::apply_edited`] registers the asset
//! and points the zones at it with **one undo step**: a `SetZones` per device, folded together
//! when the edit reaches several. The zones keep playing the same audio ([`atomik_model::remap_zone`]:
//! a crop shifts the markers, a reverse mirrors them); a zone with nothing left after a crop goes.
//!
//! The scope is the zone being edited, or with `all_uses` every zone of the project that plays
//! the same asset. Clips on tracks keep the asset they have: only zones follow. Undo restores the
//! zones and their old asset; the new asset stays in the pool for redo, and the old one is kept
//! until a save finds that neither the song nor the history needs it.

use super::*;
use atomik_model::{SampleEdit, ZoneDoc, remap_zone};
use atomik_ui::SampleOp;

/// What the app remembers of a sample edit while its worker runs.
#[derive(Debug, Clone)]
pub(super) struct EditJob {
    device: DeviceKey,
    /// The zone's index when the edit was asked for.
    zone: usize,
    all_uses: bool,
    /// The asset being edited.
    asset: String,
    op: SampleOp,
}

/// How an edit moves the positions of the sample.
fn moves(op: SampleOp) -> SampleEdit {
    match op {
        SampleOp::Crop { from, to } => SampleEdit::Crop { from, to },
        SampleOp::Reverse { from, to } => SampleEdit::Reverse { from, to },
        SampleOp::Normalise { .. } | SampleOp::FadeIn { .. } | SampleOp::FadeOut { .. } => {
            SampleEdit::Same
        }
    }
}

/// The word that names an edit in the new asset's name.
fn label(op: SampleOp) -> &'static str {
    match op {
        SampleOp::Crop { .. } => "crop",
        SampleOp::Normalise { .. } => "normalised",
        SampleOp::Reverse { .. } => "reversed",
        SampleOp::FadeIn { .. } => "fade in",
        SampleOp::FadeOut { .. } => "fade out",
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Starts the edit of the sample of zone `zone` of `device` (module docs).
    pub(super) fn edit_sample(
        &mut self,
        device: &DeviceKey,
        zone: usize,
        all_uses: bool,
        op: SampleOp,
    ) {
        let Some(asset) = self
            .doc()
            .resolve(device)
            .and_then(|d| d.zones.get(zone))
            .map(|z| z.asset.clone())
        else {
            return self.reject("The zone is gone: nothing to edit".to_owned());
        };
        let Some(frames) = self.doc().assets.get(&asset).map(|m| m.frames) else {
            return self.reject("The sample is not in the project".to_owned());
        };
        let (from, to) = match op {
            SampleOp::Crop { from, to }
            | SampleOp::Normalise { from, to }
            | SampleOp::Reverse { from, to }
            | SampleOp::FadeIn { from, to }
            | SampleOp::FadeOut { from, to } => (from, to),
        };
        if from >= to || to > frames {
            return self.reject("Select a part of the sample first".to_owned());
        }
        let Some(bundle) = self.bundle_clone() else {
            return self.reject("The sample has no file to edit yet".to_owned());
        };
        let stem: String = self.asset_name(&asset).chars().take(170).collect();
        let name = format!("{stem} ({})", label(op));
        let job = self.sampling.next_edit;
        self.sampling.next_edit += 1;
        let started = crate::audio_import::spawn_sample_edit(
            self.audio.tx.clone(),
            self.audio.generation,
            job,
            bundle,
            asset.clone(),
            op,
            name,
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.sampling.edits.insert(
                    job,
                    EditJob {
                        device: device.clone(),
                        zone,
                        all_uses,
                        asset,
                        op,
                    },
                );
            }
            Err(e) => self.reject(format!("sample edit: cannot start a worker: {e}")),
        }
    }

    /// Applies a finished edit: registers the new asset and swaps it into the zones.
    pub(super) fn apply_edited(
        &mut self,
        job: u64,
        result: Result<Box<crate::audio_import::Loaded>, String>,
    ) {
        let Some(job) = self.sampling.edits.remove(&job) else {
            return;
        };
        let loaded = match result {
            Ok(l) => l,
            Err(e) => return self.reject(format!("sample edit: {e}")),
        };
        let Some(info) = loaded.info.clone() else {
            return;
        };
        self.insert_loaded(&loaded);
        let registered = self.apply_command(Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: atomik_model::AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        });
        if registered.is_err() {
            return;
        }
        let edit = moves(job.op);
        let swaps: Vec<(DeviceKey, Vec<ZoneDoc>)> =
            self.zone_swaps(&job, &info.hash, info.frames, edit);
        if swaps.is_empty() {
            return self
                .reject("The zone changed while the edit ran: nothing was swapped".to_owned());
        }
        for (n, (device, zones)) in swaps.into_iter().enumerate() {
            // The devices after the first fold into its undo step. A rejection is reported.
            self.join_next = n > 0;
            let _ = self.apply_command(Command::SetZones {
                device,
                zones,
                merge: None,
            });
            self.join_next = false;
        }
        self.document.end_merge();
    }

    /// The new zone list of every device the edit reaches.
    fn zone_swaps(
        &self,
        job: &EditJob,
        asset: &str,
        frames: u64,
        edit: SampleEdit,
    ) -> Vec<(DeviceKey, Vec<ZoneDoc>)> {
        let mut out = Vec::new();
        let targets: Vec<(DeviceKey, &RackDevice)> = if job.all_uses {
            self.flat_devices()
        } else {
            self.doc()
                .resolve(&job.device)
                .map(|d| vec![(job.device.clone(), d)])
                .unwrap_or_default()
        };
        for (key, device) in targets {
            let mut changed = false;
            let mut zones = Vec::with_capacity(device.zones.len());
            for (i, z) in device.zones.iter().enumerate() {
                let wanted =
                    z.asset == job.asset && (job.all_uses || (key == job.device && i == job.zone));
                if wanted {
                    changed = true;
                    zones.extend(remap_zone(z, asset, frames, edit));
                } else {
                    zones.push(z.clone());
                }
            }
            if changed {
                out.push((key, zones));
            }
        }
        out
    }
}
