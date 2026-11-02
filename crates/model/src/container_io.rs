//! Reading and writing a container's `"inner"` (project format v8).
//!
//! ```text
//! "inner": { "devices": [ <device objects, local keys> ],
//!            "cables":  [ {"from": "lo:out_l", "to": "boundary:out_l"} … ],
//!            "macros":  [ {"name": "Tone", "targets": [ {"device": "fx/delay", "param": "mix",
//!                                                         "min": 0, "max": 1} … ]} × 8 ],
//!            "splits":  [ {"target": "lo", "key_lo", "key_hi", "vel_lo", "vel_hi",
//!                          "transpose"} … ],
//!            "open":    true }
//! ```
//!
//! `"open"` is optional: it is written, as the last field, only when the container is unfolded
//! in the rack (a presentation flag); a missing one means folded. Inner devices and cables are in
//! the same canonical form as at the top level (cable ends
//! are local keys; the container's inside edge is `boundary`), `macros` always has 8 entries.
//! The reader checks shapes while it reads; the rules (limits, keys, cables, splits, macros)
//! are those of [`crate::RackDoc::check_containers`], run once on the loaded project.

use crate::container::{
    ContainerDoc, MACRO_COUNT, MAX_CONTAINER_DEPTH, MacroDoc, MacroTarget, NoteSplit, Seg,
    Violation,
};
use crate::patch::{
    DeviceFormat, JsonPath, PluginUse, array, endpoint, f32_text, json_str, load_device, number,
    object, string, uint, write_device,
};
use crate::{Cable, DeviceKey, ModelError, RackDevice};
use atomik_device_sdk::Registry;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;

const INNER_FIELDS: &[&str] = &["devices", "cables", "macros", "splits"];
const INNER_ALLOWED: &[&str] = &["devices", "cables", "macros", "splits", "open"];
const CABLE_FIELDS: &[&str] = &["from", "to"];
const MACRO_FIELDS: &[&str] = &["name", "targets"];
const TARGET_FIELDS: &[&str] = &["device", "param", "min", "max"];
const SPLIT_FIELDS: &[&str] = &[
    "target",
    "key_lo",
    "key_hi",
    "vel_lo",
    "vel_hi",
    "transpose",
];

/// The JSON path of a violation found by the container check, below `root`.
pub(crate) fn violation_error(root: &JsonPath, v: &Violation) -> ModelError {
    let mut path = root.clone();
    for seg in &v.path {
        path = match seg {
            Seg::F(name) => path.field(name),
            Seg::I(i) => path.index(*i),
        };
    }
    path.error(v.message())
}

/// The error for a container nested too deep, at the `inner` field of the offending device.
pub(crate) fn too_deep(path: &JsonPath) -> ModelError {
    path.error(format!(
        "containers nest at most {MAX_CONTAINER_DEPTH} levels deep"
    ))
}

/// Reads a container's `"inner"` object. `depth` is the container's own depth (1 at the top
/// level). Rules beyond shapes are checked later, on the whole project.
pub(crate) fn load_inner(
    v: &Value,
    path: &JsonPath,
    depth: usize,
    fmt: DeviceFormat,
    registry: &Registry,
) -> Result<ContainerDoc, ModelError> {
    let o = object(v, path, INNER_ALLOWED, INNER_FIELDS)?;

    let devices_path = path.field("devices");
    let mut devices: Vec<RackDevice> = Vec::new();
    let mut first_use: BTreeMap<String, JsonPath> = BTreeMap::new();
    for (i, item) in array(&o["devices"], &devices_path)?.iter().enumerate() {
        let p = devices_path.index(i);
        let device = load_device(item, &p, fmt, depth, &devices, registry, &first_use)?;
        first_use.insert(device.key.0.clone(), p.field("id"));
        devices.push(device);
    }

    let cables_path = path.field("cables");
    let mut cables = Vec::new();
    for (i, item) in array(&o["cables"], &cables_path)?.iter().enumerate() {
        let p = cables_path.index(i);
        let c = object(item, &p, CABLE_FIELDS, CABLE_FIELDS)?;
        cables.push(Cable {
            from: endpoint(&c["from"], &p.field("from"))?,
            to: endpoint(&c["to"], &p.field("to"))?,
        });
    }

    let macros_path = path.field("macros");
    let list = array(&o["macros"], &macros_path)?;
    if list.len() != MACRO_COUNT {
        return Err(macros_path.error(format!(
            "expected {MACRO_COUNT} macros, found {}",
            list.len()
        )));
    }
    let mut macros: [MacroDoc; MACRO_COUNT] = Default::default();
    for (i, item) in list.iter().enumerate() {
        let p = macros_path.index(i);
        let m = object(item, &p, MACRO_FIELDS, MACRO_FIELDS)?;
        let targets_path = p.field("targets");
        let mut targets = Vec::new();
        for (j, t) in array(&m["targets"], &targets_path)?.iter().enumerate() {
            let tp = targets_path.index(j);
            let t = object(t, &tp, TARGET_FIELDS, TARGET_FIELDS)?;
            targets.push(MacroTarget {
                device: DeviceKey(string(&t["device"], &tp.field("device"))?.to_owned()),
                param: string(&t["param"], &tp.field("param"))?.to_owned(),
                min: number(&t["min"], &tp.field("min"))? as f32,
                max: number(&t["max"], &tp.field("max"))? as f32,
            });
        }
        macros[i] = MacroDoc {
            name: string(&m["name"], &p.field("name"))?.to_owned(),
            targets,
        };
    }

    let splits_path = path.field("splits");
    let mut splits = Vec::new();
    for (i, item) in array(&o["splits"], &splits_path)?.iter().enumerate() {
        let p = splits_path.index(i);
        let s = object(item, &p, SPLIT_FIELDS, SPLIT_FIELDS)?;
        let byte = |name: &str| uint(&s[name], &p.field(name), 255).map(|n| n as u8);
        let transpose_path = p.field("transpose");
        let transpose = s["transpose"].as_i64().ok_or_else(|| {
            transpose_path.error(format!("expected an integer, found {}", s["transpose"]))
        })?;
        let transpose = i8::try_from(transpose)
            .map_err(|_| transpose_path.error(format!("{transpose} is outside -48..=48")))?;
        splits.push(NoteSplit {
            target: DeviceKey(string(&s["target"], &p.field("target"))?.to_owned()),
            key_lo: byte("key_lo")?,
            key_hi: byte("key_hi")?,
            vel_lo: byte("vel_lo")?,
            vel_hi: byte("vel_hi")?,
            transpose,
        });
    }

    let open = match o.get("open") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| {
            path.field("open")
                .error(format!("expected true or false, found {v}"))
        })?,
    };

    Ok(ContainerDoc {
        devices,
        cables,
        macros,
        splits,
        open,
    })
}

/// Writes `"inner": { … }` (no leading newline or comma) for a device object whose fields are
/// at indent `p`.
pub(crate) fn write_inner(
    w: &mut String,
    inner: &ContainerDoc,
    p: &str,
    path: &str,
    used: &PluginUse,
) {
    let q = format!("{p}  ");
    let r = format!("{p}    ");
    let _ = write!(w, "{{\n{q}\"devices\": [");
    for (i, d) in inner.devices.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = writeln!(w, "{sep}");
        write_device(w, d, &r, true, &format!("{path}/{}", d.key.0), used);
    }
    let _ = write!(
        w,
        "{}],\n{q}\"cables\": [",
        if inner.devices.is_empty() {
            String::new()
        } else {
            format!("\n{q}")
        }
    );
    for (i, c) in inner.cables.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n{r}{{\"from\": {}, \"to\": {}}}",
            json_str(&c.from.to_string()),
            json_str(&c.to.to_string())
        );
    }
    let _ = write!(
        w,
        "{}],\n{q}\"macros\": [",
        if inner.cables.is_empty() {
            String::new()
        } else {
            format!("\n{q}")
        }
    );
    for (i, m) in inner.macros.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n{r}{{\"name\": {}, \"targets\": [",
            json_str(&m.name)
        );
        for (j, t) in m.targets.iter().enumerate() {
            let sep = if j == 0 { "" } else { "," };
            let _ = write!(
                w,
                "{sep}\n{r}  {{\"device\": {}, \"param\": {}, \"min\": {}, \"max\": {}}}",
                json_str(&t.device.0),
                json_str(&t.param),
                f32_text(t.min),
                f32_text(t.max)
            );
        }
        let _ = write!(
            w,
            "{}]}}",
            if m.targets.is_empty() {
                String::new()
            } else {
                format!("\n{r}")
            }
        );
    }
    let _ = write!(w, "\n{q}],\n{q}\"splits\": [");
    for (i, s) in inner.splits.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n{r}{{\"target\": {}, \"key_lo\": {}, \"key_hi\": {}, \"vel_lo\": {}, \"vel_hi\": {}, \"transpose\": {}}}",
            json_str(&s.target.0),
            s.key_lo,
            s.key_hi,
            s.vel_lo,
            s.vel_hi,
            s.transpose
        );
    }
    let _ = write!(
        w,
        "{}]",
        if inner.splits.is_empty() {
            String::new()
        } else {
            format!("\n{q}")
        }
    );
    if inner.open {
        let _ = write!(w, ",\n{q}\"open\": true");
    }
    let _ = write!(w, "\n{p}}}");
}

/// Puts the inner cables of every container into canonical (sorted) order. The reader checks
/// them in file order, so a fault is reported at the index the file has.
pub(crate) fn sort_inner_cables(devices: &mut [RackDevice]) {
    for d in devices {
        if let Some(inner) = &mut d.inner {
            inner.cables.sort();
            sort_inner_cables(&mut inner.devices);
        }
    }
}
