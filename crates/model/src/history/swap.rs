//! The edits that swap one value for another: a field of a device, of a track or of the
//! document. They share one inverse (`old` and `new` swap), so a new one is one entry in the
//! table below: its name, what it is keyed by, the type of the value and how to store it.

use super::{InvariantError, device_mut, fail, lane_mut, track_mut};
use crate::song_info::SongInfo;
use crate::{DeviceKey, RackDoc, SyncOp};

macro_rules! swaps {
    ($(
        $(#[$meta:meta])*
        $variant:ident { $($key:ident : $key_ty:ty),* } : $value:ty
            => |$doc:ident, $new:ident| $body:expr;
    )*) => {
        /// A value swapped for another (see the module docs). `old` is what the document holds
        /// and `new` what the edit stores; the inverse is the same edit the other way round.
        #[derive(Debug, Clone, PartialEq)]
        pub(crate) enum Swap {
            $(
                $(#[$meta])*
                $variant { $($key: $key_ty,)* old: $value, new: $value },
            )*
        }

        impl Swap {
            /// Turns the swap into the one that undoes it.
            pub(crate) fn invert(&mut self) {
                match self {
                    $(Self::$variant { old, new, .. } => std::mem::swap(old, new),)*
                }
            }

            /// Stores the new value; the document is unchanged when it refuses.
            pub(crate) fn perform(
                &self,
                doc: &mut RackDoc,
            ) -> Result<Vec<SyncOp>, InvariantError> {
                match self {
                    $(Self::$variant { $($key,)* new, .. } => {
                        let store = |$doc: &mut RackDoc,
                                     $new: &$value|
                         -> Result<Vec<SyncOp>, InvariantError> { $body };
                        store(doc, new)
                    })*
                }
            }
        }
    };
}

swaps! {
    /// The number of rack columns.
    Columns {}: u16 => |doc, new| {
        doc.columns = *new;
        Ok(vec![SyncOp::ViewChanged])
    };
    /// Whether a device is folded.
    Folded { key: DeviceKey }: bool => |doc, new| {
        device_mut(doc, key)?.folded = *new;
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The name of a device.
    Name { key: DeviceKey }: String => |doc, new| {
        device_mut(doc, key)?.name.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The display name of a pool asset.
    AssetName { asset: String }: String => |doc, new| {
        match doc.assets.get_mut(asset) {
            Some(meta) => meta.original_name.clone_from(new),
            None => return fail(format!("there is no asset {asset}")),
        }
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The colour of a device.
    Color { key: DeviceKey }: Option<[u8; 3]> => |doc, new| {
        device_mut(doc, key)?.color = *new;
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The patch name a device shows (empty is none).
    PatchName { key: DeviceKey }: String => |doc, new| {
        device_mut(doc, key)?.patch_name.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The MIDI port a MIDI Out device sends to (`None` is the settings' fallback port).
    MidiOutPort { key: DeviceKey }: Option<String> => |doc, new| {
        device_mut(doc, key)?.midi_out = new.as_deref().map(crate::MidiOutDoc::new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// A device's modulation slots (see [`crate::ModSlot`]).
    ModSlots { key: DeviceKey }: Vec<Option<crate::ModSlot>> => |doc, new| {
        device_mut(doc, key)?.mods.clone_from(new);
        Ok(vec![SyncOp::ModSlotsChanged { device: key.clone() }])
    };
    /// The song information (title, author, notes, address, e-mail).
    Info {}: SongInfo => |doc, new| {
        doc.info.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The reference tracks (see [`crate::Reference`]).
    References {}: Vec<crate::Reference> => |doc, new| {
        doc.references.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The delivery target (see [`crate::Delivery`]).
    Delivery {}: Option<crate::Delivery> => |doc, new| {
        doc.delivery.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The project's own camera settings (see [`crate::CameraOverride`]).
    Camera {}: Option<crate::CameraOverride> => |doc, new| {
        doc.camera.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// Which insert areas of the rack are folded (see [`crate::meter_view`]).
    InsertsFolded {}: Vec<DeviceKey> => |doc, new| {
        doc.folded_inserts.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// What each Spectrum device draws as its second curve (see [`crate::meter_view`]).
    SpectrumCurves {}: Vec<crate::SpectrumCurve> => |doc, new| {
        doc.spectrum_curves.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The song's video (see [`crate::VideoClip`]).
    Video {}: Option<crate::VideoClip> => |doc, new| {
        doc.video.clone_from(new);
        Ok(vec![SyncOp::ViewChanged])
    };
    /// The master tune in cents.
    Tune {}: f32 => |doc, new| {
        doc.tune_cents = *new;
        Ok(vec![SyncOp::TuneChanged(*new)])
    };
    /// How the rack arranges its devices.
    RackSort {}: crate::RackSort => |doc, new| {
        doc.sort_mode = *new;
        Ok(vec![SyncOp::ViewChanged])
    };
    /// Track chase flag.
    TrackChase { device: DeviceKey }: bool => |doc, new| {
        track_mut(doc, device)?.chase = *new;
        Ok(vec![SyncOp::SongChanged])
    };
    /// Track Direct Record flag.
    TrackDirectRecord { device: DeviceKey }: bool => |doc, new| {
        track_mut(doc, device)?.direct_record = *new;
        Ok(vec![SyncOp::SongChanged])
    };
    /// Track live groove.
    TrackGroove { device: DeviceKey }: Option<crate::song::TrackGroove> => |doc, new| {
        track_mut(doc, device)?.groove.clone_from(new);
        Ok(vec![SyncOp::SongChanged])
    };
    /// A track's own controller lanes (song ticks).
    TrackControllers { device: DeviceKey }: Vec<crate::controller::ControllerLane> => |doc, new| {
        track_mut(doc, device)?.controllers.clone_from(new);
        Ok(vec![SyncOp::SongChanged])
    };
    /// Whether a note lane is soloed.
    LaneSolo { device: DeviceKey, lane: usize }: bool => |doc, new| {
        lane_mut(doc, device, *lane)?.solo = *new;
        Ok(vec![SyncOp::SongChanged])
    };
    /// The colour of a note lane (`None`: the track's).
    LaneColor { device: DeviceKey, lane: usize }: Option<[u8; 3]> => |doc, new| {
        lane_mut(doc, device, *lane)?.color = *new;
        Ok(vec![SyncOp::SongChanged])
    };
    /// Where a track's notes enter (`None`: its own device).
    TrackEntry { device: DeviceKey }: Option<DeviceKey> => |doc, new| {
        track_mut(doc, device)?.notes_to.clone_from(new);
        Ok(vec![SyncOp::SongChanged])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::empty_doc;

    #[test]
    fn a_swap_stores_the_new_value_and_its_inverse_restores_the_old_one() {
        let mut doc = empty_doc();
        let before = doc.clone();
        let mut columns = Swap::Columns { old: 1, new: 3 };
        columns.perform(&mut doc).unwrap();
        assert_eq!(doc.columns, 3);
        columns.invert();
        columns.perform(&mut doc).unwrap();
        assert_eq!(doc, before);
        columns.invert();
        assert_eq!(
            columns,
            Swap::Columns { old: 1, new: 3 },
            "inverting twice is no change"
        );
    }

    #[test]
    fn a_swap_that_names_nothing_is_refused_and_changes_nothing() {
        let mut doc = empty_doc();
        let before = doc.clone();
        let ghost = DeviceKey("ghost".into());
        let named = Swap::Name {
            key: ghost.clone(),
            old: String::new(),
            new: "x".into(),
        };
        assert!(named.perform(&mut doc).is_err());
        let chase = Swap::TrackChase {
            device: ghost,
            old: true,
            new: false,
        };
        assert!(chase.perform(&mut doc).is_err());
        let asset = Swap::AssetName {
            asset: "missing".into(),
            old: String::new(),
            new: "x".into(),
        };
        assert!(asset.perform(&mut doc).is_err());
        assert_eq!(doc, before);
    }
}
