//! Writing the song's optional `"launcher"` object (see [`crate::launcher`]).

use super::write_clip::{list, write_clip};
use super::*;
use crate::launcher::LaunchQuantize;

/// Writes `"launcher": {...},` when the song's launcher holds anything. Slots of a device that
/// has no track are left out.
pub(super) fn write_launcher(w: &mut String, song: &Song) {
    let l = &song.launcher;
    if l.scenes.is_empty() && l.quantize == LaunchQuantize::Bar && l.stop_empty {
        return;
    }
    let slots: Vec<_> = l
        .clips()
        .filter(|(d, _, _)| song.tracks.iter().any(|t| &t.device == *d))
        .collect();
    let _ = write!(w, "    \"launcher\": {{");
    let mut first = true;
    let mut sep = |w: &mut String| {
        w.push_str(if first { "\n      " } else { ",\n      " });
        first = false;
    };
    if l.quantize != LaunchQuantize::Bar {
        sep(w);
        let _ = write!(w, "\"quantize\": {}", json_str(l.quantize.key()));
    }
    if !l.stop_empty {
        sep(w);
        w.push_str("\"stop_empty\": false");
    }
    if !l.scenes.is_empty() {
        sep(w);
        let _ = write!(w, "\"scenes\": ");
        list(
            w,
            "      ",
            l.scenes.iter().map(|s| {
                let mut parts: Vec<String> = Vec::new();
                if !s.name.is_empty() {
                    parts.push(format!("\"name\": {}", json_str(&s.name)));
                }
                if let Some([r, g, b]) = s.color {
                    parts.push(format!("\"color\": \"#{r:02x}{g:02x}{b:02x}\""));
                }
                if let Some(t) = s.tempo {
                    parts.push(format!("\"tempo\": {t}"));
                }
                format!("{{{}}}", parts.join(", "))
            }),
        );
    }
    if !slots.is_empty() {
        sep(w);
        let _ = write!(w, "\"slots\": [");
        for (i, (device, row, slot)) in slots.iter().enumerate() {
            let comma = if i == 0 { "" } else { "," };
            let q = slot.quantize.map_or(String::new(), |q| {
                format!(", \"quantize\": {}", json_str(q.key()))
            });
            let _ = write!(
                w,
                "{comma}\n        {{\"track\": {}, \"row\": {row}{q}, \"clip\":",
                json_str(&device.0)
            );
            write_clip(w, &slot.clip, "");
            w.push('}');
        }
        w.push_str("\n      ]");
    }
    w.push_str("\n    },\n");
}
