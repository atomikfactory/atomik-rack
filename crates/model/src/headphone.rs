//! The Headphone Correction device's imported data.
//!
//! The device (`atomik.headphone-correction`) corrects the response of the user's headphones from
//! data the user imports: a frequency response or a parametric EQ text (see
//! [`atomik_device_sdk::headphone`]). What was read is document data of the device, like a
//! plugin's state: [`RackDevice::headphone`] is `Some` once something was imported and `None`
//! while the device has no data (then it passes the signal through). The value is stored in the
//! project itself, so a project does not depend on the file the data came from; its
//! [`HeadphoneData::name`] is a display name only.
//!
//! The field is stored only when set (project format 45, schema row `headphone`) and only on a
//! type registered with `Registry::register_with_headphone`. The engine gets the same value as
//! [`atomik_device_sdk::PlayerData::Headphone`] from [`crate::player_data_for`].
//!
//! # Stored form
//!
//! ```text
//! "headphone": { "name": string,
//!                "response": { "freq": [Hz…], "left": [dB…], "right"?: [dB…] }       (a response)
//!              | "filters": { "preamp": dB, "bands": [{ "type": "PK"|"LS"|"HS"|"LP"|"HP",
//!                                                       "freq": Hz, "gain": dB, "q": number }…] } }
//! ```
//!
//! Exactly one of `response` and `filters`. The limits are those of
//! [`HeadphoneData::validate`] (2 to 4096 points with rising frequencies, at most 64 bands).

use crate::history::Edit;
use crate::patch::{JsonPath, array, describe, f32_text, json_str, number, object, string};
use crate::{DeviceKey, ModelError, RackDevice, RackDoc, rules};
use atomik_device_sdk::Registry;
use atomik_device_sdk::headphone::{
    Filter, FilterData, FilterKind, HeadphoneBody, HeadphoneData, MAX_FILTERS, MAX_POINTS,
    ResponseData,
};
use serde_json::Value;
use std::fmt::Write as _;

/// Most devices with data in one project file.
pub const MAX_DEVICES: usize = 256;

/// Plans `SetHeadphoneData`: the data of a Headphone Correction device (`None` removes it).
pub(crate) fn plan_set(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
    data: Option<&HeadphoneData>,
) -> Result<Edit, ModelError> {
    let device = rules::resolve(doc, key)?;
    if !registry.takes_headphone(&device.type_id) {
        return Err(ModelError::BadScope(format!(
            "{key} is not a Headphone Correction device"
        )));
    }
    if let Some(d) = data {
        d.validate()
            .map_err(|m| ModelError::BadScope(format!("headphone data: {m}")))?;
    }
    let old = device.headphone.clone();
    if old.as_ref() == data {
        return Err(ModelError::BadScope(format!(
            "{key} already has that headphone data"
        )));
    }
    Ok(Edit::Headphone {
        key: key.clone(),
        old,
        new: data.cloned(),
    })
}

impl RackDevice {
    /// The headphone data of a Headphone Correction device, if it has any.
    pub fn headphone_data(&self) -> Option<&HeadphoneData> {
        self.headphone.as_ref()
    }
}

/// Reads a device's `"headphone"` object. Whether the device may have one is checked by the
/// caller, which knows the registry.
pub(crate) fn load(v: &Value, path: &JsonPath) -> Result<HeadphoneData, ModelError> {
    let o = object(v, path, &["name", "response", "filters"], &["name"])?;
    let name = string(&o["name"], &path.field("name"))?.to_owned();
    let body = match (o.get("response"), o.get("filters")) {
        (Some(r), None) => HeadphoneBody::Response(load_response(r, &path.field("response"))?),
        (None, Some(f)) => HeadphoneBody::Filters(load_filters(f, &path.field("filters"))?),
        _ => {
            return Err(path.error("expected exactly one of \"response\" and \"filters\""));
        }
    };
    let data = HeadphoneData { name, body };
    data.validate().map_err(|m| path.error(m))?;
    Ok(data)
}

fn numbers(v: &Value, path: &JsonPath, max: usize) -> Result<Vec<f32>, ModelError> {
    let list = array(v, path)?;
    if list.len() > max {
        return Err(path.error(format!("{} entries, at most {max} are allowed", list.len())));
    }
    list.iter()
        .enumerate()
        .map(|(i, x)| number(x, &path.index(i)).map(|n| n as f32))
        .collect()
}

fn load_response(v: &Value, path: &JsonPath) -> Result<ResponseData, ModelError> {
    let o = object(v, path, &["freq", "left", "right"], &["freq", "left"])?;
    let freq = numbers(&o["freq"], &path.field("freq"), MAX_POINTS)?;
    let left = numbers(&o["left"], &path.field("left"), MAX_POINTS)?;
    let right = match o.get("right") {
        Some(r) => Some(numbers(r, &path.field("right"), MAX_POINTS)?),
        None => None,
    };
    Ok(ResponseData { freq, left, right })
}

fn load_filters(v: &Value, path: &JsonPath) -> Result<FilterData, ModelError> {
    let o = object(v, path, &["preamp", "bands"], &["preamp", "bands"])?;
    let preamp_db = number(&o["preamp"], &path.field("preamp"))? as f32;
    let bands_path = path.field("bands");
    let list = array(&o["bands"], &bands_path)?;
    if list.len() > MAX_FILTERS {
        return Err(bands_path.error(format!(
            "{} bands, at most {MAX_FILTERS} are allowed",
            list.len()
        )));
    }
    let mut filters = Vec::with_capacity(list.len());
    for (i, b) in list.iter().enumerate() {
        let p = bands_path.index(i);
        let o = object(
            b,
            &p,
            &["type", "freq", "gain", "q"],
            &["type", "freq", "gain", "q"],
        )?;
        let type_path = p.field("type");
        let name = string(&o["type"], &type_path)?;
        let kind = FilterKind::from_name(name).ok_or_else(|| {
            type_path.error(format!(
                "unknown band type {name:?}; expected one of: PK, LS, HS, LP, HP"
            ))
        })?;
        filters.push(Filter {
            kind,
            freq: number(&o["freq"], &p.field("freq"))? as f32,
            gain_db: number(&o["gain"], &p.field("gain"))? as f32,
            q: number(&o["q"], &p.field("q"))? as f32,
        });
    }
    Ok(FilterData { preamp_db, filters })
}

/// Reads the root `"headphone"` map and attaches each entry to its device.
pub(crate) fn load_all(
    v: &Value,
    path: &JsonPath,
    doc: &mut RackDoc,
    registry: &Registry,
) -> Result<(), ModelError> {
    let map = v
        .as_object()
        .ok_or_else(|| path.error(format!("expected an object, found {}", describe(v))))?;
    if map.len() > MAX_DEVICES {
        return Err(path.error(format!(
            "{} devices, at most {MAX_DEVICES} are allowed",
            map.len()
        )));
    }
    for (key, value) in map {
        let at = path.field(key);
        let data = load(value, &at)?;
        let device = doc
            .resolve_mut(&DeviceKey(key.clone()))
            .ok_or_else(|| at.error("there is no device with this key"))?;
        if !registry.takes_headphone(&device.type_id) {
            return Err(at.error(format!("{} does not take headphone data", device.type_id)));
        }
        device.headphone = Some(data);
    }
    Ok(())
}

fn collect<'d>(
    devices: &'d [RackDevice],
    prefix: &str,
    out: &mut Vec<(String, &'d HeadphoneData)>,
) {
    for d in devices {
        let path = if prefix.is_empty() {
            d.key.0.clone()
        } else {
            format!("{prefix}/{}", d.key.0)
        };
        if let Some(h) = &d.headphone {
            out.push((path.clone(), h));
        }
        if let Some(inner) = &d.inner {
            collect(&inner.devices, &path, out);
        }
    }
}

/// Writes the root `"headphone"` map (with its trailing comma and newline); nothing when no
/// device has data.
pub(crate) fn write_all(w: &mut String, doc: &RackDoc) {
    let mut found = Vec::new();
    collect(&doc.devices, "", &mut found);
    if found.is_empty() {
        return;
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let _ = write!(w, "  \"headphone\": {{");
    for (i, (key, data)) in found.iter().enumerate() {
        let _ = write!(
            w,
            "{}
    {}: ",
            if i == 0 { "" } else { "," },
            json_str(key)
        );
        write(w, data, "    ");
    }
    let _ = writeln!(
        w,
        "
  }},"
    );
}

fn write_numbers(w: &mut String, v: &[f32]) {
    w.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            w.push_str(", ");
        }
        w.push_str(&f32_text(*x));
    }
    w.push(']');
}

/// Writes the `"headphone"` object (braces at indent `ind`).
pub(crate) fn write(w: &mut String, d: &HeadphoneData, ind: &str) {
    let p = format!("{ind}  ");
    let _ = write!(w, "{{\n{p}\"name\": {},\n", json_str(&d.name));
    match &d.body {
        HeadphoneBody::Response(r) => {
            let _ = write!(w, "{p}\"response\": {{\n{p}  \"freq\": ");
            write_numbers(w, &r.freq);
            let _ = write!(w, ",\n{p}  \"left\": ");
            write_numbers(w, &r.left);
            if let Some(right) = &r.right {
                let _ = write!(w, ",\n{p}  \"right\": ");
                write_numbers(w, right);
            }
            let _ = write!(w, "\n{p}}}");
        }
        HeadphoneBody::Filters(f) => {
            let _ = write!(
                w,
                "{p}\"filters\": {{\n{p}  \"preamp\": {},\n{p}  \"bands\": [",
                f32_text(f.preamp_db)
            );
            for (i, b) in f.filters.iter().enumerate() {
                let _ = write!(
                    w,
                    "{}\n{p}    {{ \"type\": {}, \"freq\": {}, \"gain\": {}, \"q\": {} }}",
                    if i == 0 { "" } else { "," },
                    json_str(b.kind.name()),
                    f32_text(b.freq),
                    f32_text(b.gain_db),
                    f32_text(b.q)
                );
            }
            let _ = write!(
                w,
                "{}]\n{p}}}",
                if f.filters.is_empty() {
                    String::new()
                } else {
                    format!("\n{p}  ")
                }
            );
        }
    }
    let _ = write!(w, "\n{ind}}}");
}
