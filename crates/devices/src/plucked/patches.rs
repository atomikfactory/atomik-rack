//! Which factory patch of Plucked Strings suits each instrument of the tab catalogue.
//!
//! The ids are the plain strings of the guitar-tab instrument catalogue (`guitar6`, `banjo5`,
//! ...), kept here as text so the device crate needs nothing from the tab crate and the app can
//! ask for the patch to load on a track that plays a given instrument. The names are those of
//! the factory patches of the `Plucked` category (the app tests check that every one exists).

/// The patch of an instrument id that is not in [`INSTRUMENT_PATCHES`].
pub const DEFAULT_PATCH: &str = "Steel Acoustic";

/// `(instrument id, factory patch name)` for every instrument of the catalogue.
pub const INSTRUMENT_PATCHES: [(&str, &str); 24] = [
    ("guitar6", "Steel Acoustic"),
    ("guitar7", "Steel Acoustic"),
    ("guitar8", "Steel Acoustic"),
    ("guitar12", "12-String"),
    ("bass4", "Electric Bass Finger"),
    ("bass5", "Electric Bass Finger"),
    ("bass6", "Electric Bass Finger"),
    ("ukulele_soprano", "Ukulele"),
    ("ukulele_concert", "Ukulele"),
    ("ukulele_tenor", "Ukulele"),
    ("ukulele_baritone", "Ukulele"),
    ("banjo5", "Banjo"),
    ("banjo4_tenor", "Banjo"),
    ("banjo4_plectrum", "Banjo"),
    ("mandolin", "Mandolin"),
    ("mandola", "Mandolin"),
    ("octave_mandolin", "Mandolin"),
    ("mandocello", "Mandolin"),
    ("bouzouki_irish", "Bouzouki"),
    ("balalaika_prima", "Mandolin"),
    ("dobro", "Dobro"),
    ("cavaquinho", "Ukulele"),
    ("charango", "Mandolin"),
    ("lute", "Nylon Classical"),
];

/// The name of the factory patch to load for the instrument `instrument_id`: the one in
/// [`INSTRUMENT_PATCHES`], or [`DEFAULT_PATCH`] for an id it does not list.
pub fn factory_patch_for(instrument_id: &str) -> &'static str {
    INSTRUMENT_PATCHES
        .iter()
        .find(|(id, _)| *id == instrument_id)
        .map_or(DEFAULT_PATCH, |(_, patch)| patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_instrument_has_its_patch_and_an_unknown_one_gets_the_default() {
        assert_eq!(factory_patch_for("guitar12"), "12-String");
        assert_eq!(factory_patch_for("bass5"), "Electric Bass Finger");
        assert_eq!(factory_patch_for("lute"), "Nylon Classical");
        assert_eq!(factory_patch_for(""), DEFAULT_PATCH);
        assert_eq!(factory_patch_for("kazoo"), "Steel Acoustic");
        let mut ids: Vec<&str> = INSTRUMENT_PATCHES.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), INSTRUMENT_PATCHES.len(), "an id is listed once");
    }
}
