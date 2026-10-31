//! Writing the console.

use super::*;

pub(super) fn key_or_null(k: Option<&DeviceKey>) -> String {
    k.map_or_else(|| "null".to_owned(), |k| json_str(&k.0))
}

pub(super) fn write_console(w: &mut String, console: &Console) {
    let _ = writeln!(w, "  \"console\": {{");
    let _ = write!(w, "    \"channels\": ");
    list(
        w,
        "    ",
        console.channels.iter().map(|c| {
            format!(
                "{{\"channel\": {}, \"source\": {}}}",
                json_str(&c.channel.0),
                key_or_null(c.source.as_ref())
            )
        }),
    );
    let _ = writeln!(w, ",");
    if !console.buses.is_empty() {
        let _ = write!(w, "    \"buses\": ");
        list(
            w,
            "    ",
            console.buses.iter().map(|b| {
                format!(
                    "{{\"bus\": {}, \"name\": {}}}",
                    json_str(&b.bus.0),
                    json_str(&b.name)
                )
            }),
        );
        let _ = writeln!(w, ",");
    }
    if !console.vcas.is_empty() {
        let _ = write!(w, "    \"vcas\": ");
        list(
            w,
            "    ",
            console.vcas.iter().map(|v| {
                let mut out = format!("{{\"id\": {}, \"name\": {}", v.id, json_str(&v.name));
                if let Some([r, g, b]) = v.color {
                    let _ = write!(out, ", \"color\": \"#{r:02x}{g:02x}{b:02x}\"");
                }
                if v.level_db != 0.0 {
                    let _ = write!(out, ", \"level_db\": {}", f32_text(v.level_db));
                }
                if v.mute {
                    out.push_str(", \"mute\": true");
                }
                if v.solo {
                    out.push_str(", \"solo\": true");
                }
                let members: Vec<String> = v.members.iter().map(|m| json_str(&m.0)).collect();
                let _ = write!(out, ", \"members\": [{}]}}", members.join(", "));
                out
            }),
        );
        let _ = writeln!(w, ",");
    }
    if !console.snapshots.is_empty() {
        let _ = write!(w, "    \"snapshots\": ");
        list(w, "    ", console.snapshots.iter().map(snapshot_json));
        let _ = writeln!(w, ",");
    }
    let _ = writeln!(
        w,
        "    \"master\": {},",
        key_or_null(console.master.as_ref())
    );
    let returns: Vec<String> = console
        .returns
        .iter()
        .map(|r| key_or_null(r.as_ref()))
        .collect();
    let _ = writeln!(w, "    \"returns\": [{}],", returns.join(", "));
    if console.align_returns {
        let _ = writeln!(w, "    \"compensation\": {},", console.compensation);
        let _ = writeln!(w, "    \"align_returns\": true");
    } else {
        let _ = writeln!(w, "    \"compensation\": {}", console.compensation);
    }
    let _ = writeln!(w, "  }},");
}

/// One snapshot as a single JSON line.
pub(super) fn snapshot_json(s: &MixerSnapshot) -> String {
    let devices: Vec<String> = s
        .state
        .devices
        .iter()
        .map(|(key, params)| {
            let params: Vec<String> = params
                .iter()
                .map(|(p, v)| format!("{}: {}", json_str(p), f32_text(*v)))
                .collect();
            format!("{}: {{{}}}", json_str(&key.0), params.join(", "))
        })
        .collect();
    let mut out = format!(
        "{{\"id\": {}, \"name\": {}, \"devices\": {{{}}}",
        s.id,
        json_str(&s.name),
        devices.join(", ")
    );
    if !s.state.vcas.is_empty() {
        let vcas: Vec<String> = s
            .state
            .vcas
            .iter()
            .map(|v| {
                let mut g = format!("{{\"id\": {}, \"name\": {}", v.id, json_str(&v.name));
                if v.level_db != 0.0 {
                    let _ = write!(g, ", \"level_db\": {}", f32_text(v.level_db));
                }
                if v.mute {
                    g.push_str(", \"mute\": true");
                }
                if v.solo {
                    g.push_str(", \"solo\": true");
                }
                let members: Vec<String> = v.members.iter().map(|m| json_str(&m.0)).collect();
                let _ = write!(g, ", \"members\": [{}]}}", members.join(", "));
                g
            })
            .collect();
        let _ = write!(out, ", \"vcas\": [{}]", vcas.join(", "));
    }
    out.push('}');
    out
}
