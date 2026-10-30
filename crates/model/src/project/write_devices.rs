//! Writing a device's sample zones and step, curve and drum pattern data.

use super::*;

/// A device's zones as a JSON array, one zone per line; `p` is the indent of the device's
/// fields (6 at the top level).
pub(crate) fn write_zones(w: &mut String, zones: &[ZoneDoc], p: &str) {
    list(
        w,
        p,
        zones.iter().map(|z| {
            let loop_mode = match z.loop_mode {
                LoopMode::Off => "off",
                LoopMode::Forward => "forward",
                LoopMode::PingPong => "ping_pong",
            };
            let trigger = match z.trigger {
                TriggerMode::Gate => "gate",
                TriggerMode::OneShot => "one_shot",
            };
            format!(
                "{{\"asset\": {}, \"key_lo\": {}, \"key_hi\": {}, \"vel_lo\": {}, \"vel_hi\": {}, \"root_key\": {}, \"transpose\": {}, \"tune_cents\": {}, \"gain_db\": {}, \"pan\": {}, \"start\": {}, \"end\": {}, \"loop_mode\": \"{loop_mode}\", \"loop_start\": {}, \"loop_end\": {}, \"crossfade\": {}, \"trigger\": \"{trigger}\", \"reverse\": {}}}",
                json_str(&z.asset),
                z.key_lo,
                z.key_hi,
                z.vel_lo,
                z.vel_hi,
                z.root_key,
                z.transpose,
                f32_text(z.tune_cents),
                f32_text(z.gain_db),
                f32_text(z.pan),
                z.start,
                z.end,
                z.loop_start,
                z.loop_end,
                z.crossfade,
                z.reverse
            )
        }),
    );
}

/// A device's steps as a JSON object, one step per line; `ind` is the indent of the device's
/// fields (6 at the top level).
pub(crate) fn write_steps(w: &mut String, steps: &StepSeqDoc, ind: &str) {
    let (a, b, c) = (
        format!("{ind}  "),
        format!("{ind}    "),
        format!("{ind}      "),
    );
    let _ = write!(w, "{{\n{a}\"patterns\": [");
    for (i, p) in steps.patterns.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n{b}{{\n{c}\"name\": {}, \"length\": {}, \"resolution\": \"{}\", \"swing\": {},\n{c}\"steps\": ",
            json_str(&p.name),
            p.length,
            p.resolution.name(),
            p.swing
        );
        list(
            w,
            &c,
            p.steps.iter().map(|s| {
                format!(
                    "{{\"on\": {}, \"key\": {}, \"velocity\": {}, \"gate\": {}, \"prob\": {}, \"tie\": {}}}",
                    s.on, s.key, s.velocity, s.gate, s.prob, s.tie
                )
            }),
        );
        let _ = write!(w, "\n{b}}}");
    }
    let _ = write!(w, "\n{a}]\n{ind}}}");
}

/// A device's curves as a JSON object, one step per line; `ind` is the indent of the device's
/// fields (6 at the top level).
pub(crate) fn write_curves(w: &mut String, curves: &CurveDoc, ind: &str) {
    let (a, b, c) = (
        format!("{ind}  "),
        format!("{ind}    "),
        format!("{ind}      "),
    );
    let _ = write!(
        w,
        "{{
{a}\"patterns\": ["
    );
    for (i, p) in curves.patterns.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}
{b}{{"
        );
        for (r, (row, name)) in p.rows.iter().zip(CURVE_PATTERN_FIELDS).enumerate() {
            let sep = if r == 0 { "" } else { "," };
            let _ = write!(
                w,
                "{sep}
{c}\"{name}\": "
            );
            list(
                w,
                &c,
                row.iter()
                    .map(|s| format!("{{\"value\": {}, \"slide\": {}}}", s.value, s.slide)),
            );
        }
        let _ = write!(
            w,
            "
{b}}}"
        );
    }
    let _ = write!(
        w,
        "
{a}]
{ind}}}"
    );
}

/// A device's drum patterns as a JSON object, one lane per line; `ind` is the indent of the
/// device's fields (6 at the top level).
pub(crate) fn write_drums(w: &mut String, drums: &DrumDoc, ind: &str) {
    let (a, b, c) = (
        format!("{ind}  "),
        format!("{ind}    "),
        format!("{ind}      "),
    );
    let _ = write!(w, "{{\n{a}\"patterns\": [");
    for (i, p) in drums.patterns.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n{b}{{\n{c}\"name\": {}, \"length\": {}, \"resolution\": \"{}\", \"swing\": {},\n{c}\"lanes\": ",
            json_str(&p.name),
            p.length,
            p.resolution.name(),
            p.swing
        );
        let numbers = |row: &[DrumStepDoc], f: fn(&DrumStepDoc) -> u8| {
            row.iter()
                .map(|s| f(s).to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        list(
            w,
            &c,
            p.lanes.iter().zip(&p.steps).map(|(l, row)| {
                format!(
                    "{{\"key\": {}, \"role\": \"{}\", \"mute\": {}, \"velocity\": [{}], \"prob\": [{}]}}",
                    l.key,
                    l.role.name(),
                    l.mute,
                    numbers(row, |s| s.velocity),
                    numbers(row, |s| s.prob)
                )
            }),
        );
        let _ = write!(w, "\n{b}}}");
    }
    let _ = write!(w, "\n{a}]\n{ind}}}");
}
