//! Writing the song's optional harmony fields (see [`crate::harmony`]).

use super::write_clip::list;
use super::*;
use crate::harmony::Key;

fn key_fields(key: Key) -> String {
    format!(
        "\"root\": {}, \"scale\": {}",
        key.root,
        json_str(key.scale.id())
    )
}

/// Writes `"key"`, `"key_changes"`, `"chords"` and `"scales"` (each only when it holds something).
pub(super) fn write_harmony(w: &mut String, song: &Song) {
    let h = &song.harmony;
    if let Some(key) = h.key {
        let _ = writeln!(w, "    \"key\": {{{}}},", key_fields(key));
    }
    if !h.changes.is_empty() {
        let _ = write!(w, "    \"key_changes\": ");
        list(
            w,
            "    ",
            h.changes
                .iter()
                .map(|c| format!("{{\"tick\": {}, {}}}", c.tick, key_fields(c.key))),
        );
        let _ = writeln!(w, ",");
    }
    if !h.chords.is_empty() {
        let _ = write!(w, "    \"chords\": ");
        list(
            w,
            "    ",
            h.chords.iter().map(|c| {
                let bass = c.bass.map_or(String::new(), |b| format!(", \"bass\": {b}"));
                format!(
                    "{{\"tick\": {}, \"length\": {}, \"root\": {}, \"quality\": {}{bass}}}",
                    c.tick,
                    c.length,
                    c.root,
                    json_str(c.quality.id())
                )
            }),
        );
        let _ = writeln!(w, ",");
    }
    if !h.scales.is_empty() {
        let _ = write!(w, "    \"scales\": ");
        list(
            w,
            "    ",
            h.scales.iter().map(|s| {
                format!(
                    "{{\"id\": {}, \"name\": {}, \"mask\": {}}}",
                    s.id,
                    json_str(&s.name),
                    s.mask
                )
            }),
        );
        let _ = writeln!(w, ",");
    }
}
