//! Reading and writing a plugin device's `"plugin"` object (project format v9).
//!
//! ```text
//! "plugin": { "format": "clap" | "vst3", "id": "…", "name": "…", "vendor": "…", "version": "…",
//!             "path_hint": "…"?,
//!             "layout": { "main_in": null | "mono" | "stereo", "main_out": …, "sidechain": bool,
//!                         "notes_in": bool, "notes_out": bool,
//!                         "kind": "instrument" | "effect" | "note_effect",
//!                         "params": [ { "id": 42, "name": "…", "module": "…"?, "min", "max",
//!                                       "default", "stepped": bool, "steps": 0 } … ] },
//!             "state": "<base64>"?, "pinned": [ids]?, "run_on_audio_thread": bool?,
//!             "powered": bool? }
//! ```
//!
//! Written after `inner`, only on plugin devices. `format`, `id`, `name`, `vendor`, `version` and
//! `layout` are required when reading; `state` is written when the plugin has one (standard
//! base64 with padding, no line breaks, nothing else accepted), `path_hint` when known. The
//! layout rows saved are only those the project **uses**, sorted by id (see
//! [`crate::plugin`]). Violations are load errors naming their JSON path.

use crate::patch::{JsonPath, array, f64_text, json_str, number, object, string, uint};
use crate::plugin::{
    LayoutFault, MAX_PINNED_PARAMS, MAX_PLUGIN_STATE_BYTES, MAX_PROJECT_PLUGIN_STATE_BYTES,
    PluginDoc, channels_name, kind_name, validate_layout,
};
use crate::{ModelError, RackDevice};
use atomik_device_sdk::Registry;
use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
    PluginStateBlob, descriptor_for, is_plugin_type_id,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

const PLUGIN_FIELDS: &[&str] = &[
    "format",
    "id",
    "name",
    "vendor",
    "version",
    "path_hint",
    "layout",
    "state",
    "pinned",
    "run_on_audio_thread",
    "powered",
];
const PLUGIN_REQUIRED: &[&str] = &["format", "id", "name", "vendor", "version", "layout"];
const LAYOUT_FIELDS: &[&str] = &[
    "main_in",
    "main_out",
    "sidechain",
    "notes_in",
    "notes_out",
    "kind",
    "params",
];
const ROW_FIELDS: &[&str] = &[
    "id", "name", "module", "min", "max", "default", "stepped", "steps",
];
const ROW_REQUIRED: &[&str] = &["id", "name", "min", "max", "default", "stepped", "steps"];

// ------------------------------------------------------------------------------ base64

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding and no line breaks.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(char::from(ALPHABET[(n >> 18) as usize & 63]));
        out.push(char::from(ALPHABET[(n >> 12) as usize & 63]));
        out.push(if chunk.len() > 1 {
            char::from(ALPHABET[(n >> 6) as usize & 63])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(ALPHABET[n as usize & 63])
        } else {
            '='
        });
    }
    out
}

/// The byte length a base64 text decodes to, or `None` when its shape is not base64 (length not
/// a multiple of 4, more than two `=`, a `=` before the end).
pub(crate) fn base64_decoded_len(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let pad = bytes.iter().rev().take_while(|&&b| b == b'=').count();
    if pad > 2 {
        return None;
    }
    Some((bytes.len() / 4 * 3 - pad) as u64)
}

/// Decodes canonical base64: only the standard alphabet, padding only at the end, and zero
/// bits behind the data (so that encoding the result gives the text back).
pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let len = usize::try_from(base64_decoded_len(text)?).ok()?;
    let bytes = text.as_bytes();
    let mut table = [255u8; 256];
    for (i, &c) in ALPHABET.iter().enumerate() {
        table[usize::from(c)] = i as u8;
    }
    let mut out = Vec::with_capacity(len);
    for (g, group) in bytes.chunks(4).enumerate() {
        let last = (g + 1) * 4 == bytes.len();
        let pad = if last {
            group.iter().rev().take_while(|&&b| b == b'=').count()
        } else {
            0
        };
        let mut n = 0u32;
        for (i, &c) in group.iter().enumerate() {
            let v = if i >= 4 - pad {
                0
            } else {
                match table[usize::from(c)] {
                    255 => return None,
                    v => u32::from(v),
                }
            };
            n = (n << 6) | v;
        }
        // The bits behind the data must be zero.
        match pad {
            1 if n & 0xff != 0 => return None,
            2 if n & 0xffff != 0 => return None,
            _ => {}
        }
        let bytes3 = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes3[..3 - pad]);
    }
    debug_assert_eq!(out.len(), len);
    Some(out)
}

// ------------------------------------------------------------------------------ reading

fn flag(v: &Value, path: &JsonPath) -> Result<bool, ModelError> {
    v.as_bool().ok_or_else(|| {
        path.error(format!(
            "expected a boolean, found {}",
            crate::patch::describe(v)
        ))
    })
}

fn channels(v: &Value, path: &JsonPath) -> Result<Option<Channels>, ModelError> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s == "mono" => Ok(Some(Channels::Mono)),
        Value::String(s) if s == "stereo" => Ok(Some(Channels::Stereo)),
        other => Err(path.error(format!(
            "expected null, \"mono\" or \"stereo\", found {other}"
        ))),
    }
}

fn kind(v: &Value, path: &JsonPath) -> Result<PluginKindHint, ModelError> {
    match v.as_str() {
        Some("instrument") => Ok(PluginKindHint::Instrument),
        Some("effect") => Ok(PluginKindHint::Effect),
        Some("note_effect") => Ok(PluginKindHint::NoteEffect),
        _ => Err(path.error(format!(
            "expected \"instrument\", \"effect\" or \"note_effect\", found {v}"
        ))),
    }
}

fn layout_error(path: &JsonPath, params_path: &JsonPath, f: &LayoutFault) -> ModelError {
    match f.row {
        Some(i) => params_path.index(i).error(&f.message),
        None => path.error(&f.message),
    }
}

/// Reads a `"layout"` object for `plugin` (shape and the rules of [`validate_layout`]); `name`,
/// `vendor` and `version` come from the enclosing object.
fn load_layout(
    v: &Value,
    path: &JsonPath,
    plugin: &PluginRef,
    (name, vendor, version): (String, String, String),
) -> Result<PluginLayout, ModelError> {
    let o = object(v, path, LAYOUT_FIELDS, LAYOUT_FIELDS)?;
    let main_in = channels(&o["main_in"], &path.field("main_in"))?;
    let main_out = channels(&o["main_out"], &path.field("main_out"))?;
    let sidechain = flag(&o["sidechain"], &path.field("sidechain"))?;
    let notes_in = flag(&o["notes_in"], &path.field("notes_in"))?;
    let notes_out = flag(&o["notes_out"], &path.field("notes_out"))?;
    let kind_hint = kind(&o["kind"], &path.field("kind"))?;
    let params_path = path.field("params");
    let mut params = Vec::new();
    for (i, item) in array(&o["params"], &params_path)?.iter().enumerate() {
        let p = params_path.index(i);
        let row = object(item, &p, ROW_FIELDS, ROW_REQUIRED)?;
        let u32_of = |field: &str| uint(&row[field], &p.field(field), u64::from(u32::MAX));
        params.push(PluginParamInfo {
            id: u32_of("id")? as u32,
            name: string(&row["name"], &p.field("name"))?.to_owned(),
            module: match row.get("module") {
                Some(m) => string(m, &p.field("module"))?.to_owned(),
                None => String::new(),
            },
            min: number(&row["min"], &p.field("min"))?,
            max: number(&row["max"], &p.field("max"))?,
            default: number(&row["default"], &p.field("default"))?,
            stepped: flag(&row["stepped"], &p.field("stepped"))?,
            steps: u32_of("steps")? as u32,
        });
    }
    let layout = PluginLayout {
        name,
        vendor,
        version,
        kind_hint,
        main_in,
        main_out,
        sidechain,
        notes_in,
        notes_out,
        params,
    };
    validate_layout(plugin, &layout).map_err(|f| layout_error(path, &params_path, &f))?;
    Ok(layout)
}

/// The (format, id) the device's type id names, checked against the object's own.
fn check_ref(
    o: &serde_json::Map<String, Value>,
    path: &JsonPath,
    type_id: &str,
) -> Result<PluginRef, ModelError> {
    let expected = PluginRef::from_type_id(type_id)
        .ok_or_else(|| path.error(format!("{type_id:?} is not a valid plugin type id")))?;
    let format_path = path.field("format");
    let format = match string(&o["format"], &format_path)? {
        "clap" => PluginFormat::Clap,
        "vst3" => PluginFormat::Vst3,
        other => {
            return Err(
                format_path.error(format!("expected \"clap\" or \"vst3\", found {other:?}"))
            );
        }
    };
    if format != expected.format {
        return Err(format_path.error(format!(
            "{:?} does not match the device type {type_id:?}",
            format.name()
        )));
    }
    let id_path = path.field("id");
    let id = string(&o["id"], &id_path)?;
    if id != expected.id {
        return Err(id_path.error(format!("{id:?} does not match the device type {type_id:?}")));
    }
    Ok(expected)
}

/// What a plugin object gives without its state: used to build a stand-in descriptor.
struct Header {
    plugin: PluginRef,
    layout: PluginLayout,
}

fn load_header(
    o: &serde_json::Map<String, Value>,
    path: &JsonPath,
    type_id: &str,
) -> Result<Header, ModelError> {
    let plugin = check_ref(o, path, type_id)?;
    let text = |field: &str| -> Result<String, ModelError> {
        Ok(string(&o[field], &path.field(field))?.to_owned())
    };
    let names = (text("name")?, text("vendor")?, text("version")?);
    let layout = load_layout(&o["layout"], &path.field("layout"), &plugin, names)?;
    Ok(Header { plugin, layout })
}

/// Reads the `"plugin"` object of the device of type `type_id`.
pub(crate) fn load_plugin(
    v: &Value,
    path: &JsonPath,
    type_id: &str,
) -> Result<PluginDoc, ModelError> {
    let o = object(v, path, PLUGIN_FIELDS, PLUGIN_REQUIRED)?;
    let Header { plugin, layout } = load_header(o, path, type_id)?;
    let path_hint = match o.get("path_hint") {
        Some(h) => Some(string(h, &path.field("path_hint"))?.to_owned()),
        None => None,
    };
    let state = match o.get("state") {
        Some(s) => {
            let p = path.field("state");
            let text = string(s, &p)?;
            let not_base64 =
                || p.error("not canonical base64 (standard alphabet, padded, no line breaks)");
            let len = base64_decoded_len(text).ok_or_else(not_base64)?;
            if len > MAX_PLUGIN_STATE_BYTES {
                return Err(p.error(format!(
                    "the state is {len} bytes; the limit is 16 MiB per plugin"
                )));
            }
            let bytes = base64_decode(text).ok_or_else(not_base64)?;
            Some(Arc::new(PluginStateBlob(bytes)))
        }
        None => None,
    };
    let pinned_path = path.field("pinned");
    let pinned = match o.get("pinned") {
        Some(list) => {
            let items = array(list, &pinned_path)?;
            if items.len() > MAX_PINNED_PARAMS {
                return Err(pinned_path.error(format!(
                    "at most {MAX_PINNED_PARAMS} params can be pinned, found {}",
                    items.len()
                )));
            }
            let mut ids: Vec<u32> = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let p = pinned_path.index(i);
                let id = uint(item, &p, u64::from(u32::MAX))? as u32;
                if ids.contains(&id) {
                    return Err(p.error(format!("duplicate pinned param {id}")));
                }
                ids.push(id);
            }
            ids
        }
        None => Vec::new(),
    };
    let flag_or = |field: &str, default: bool| -> Result<bool, ModelError> {
        match o.get(field) {
            Some(v) => flag(v, &path.field(field)),
            None => Ok(default),
        }
    };
    Ok(PluginDoc {
        format: plugin.format,
        id: plugin.id,
        path_hint,
        layout,
        state,
        pinned,
        run_on_audio_thread: flag_or("run_on_audio_thread", false)?,
        powered: flag_or("powered", true)?,
    })
}

/// Checks the total of all plugin states of the project, naming the state that crosses
/// the 128 MiB line. `path` is the JSON path of the `devices` array being walked.
pub(crate) fn check_state_total(
    devices: &[RackDevice],
    path: &JsonPath,
    total: &mut u64,
) -> Result<(), ModelError> {
    for (i, d) in devices.iter().enumerate() {
        let p = path.index(i);
        if let Some(s) = d.plugin.as_ref().and_then(|p| p.state.as_ref()) {
            *total += s.0.len() as u64;
            if *total > MAX_PROJECT_PLUGIN_STATE_BYTES {
                return Err(p.field("plugin").field("state").error(format!(
                    "the plugin states of the project total {total} bytes; the limit is 128 MiB"
                )));
            }
        }
        if let Some(inner) = &d.inner {
            check_state_total(&inner.devices, &p.field("inner").field("devices"), total)?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------ stand-ins

/// A registry like `registry` plus, for every plugin device of the file that `registry` does not
/// know, a descriptor built from the saved layout; `None` when there is nothing to add.
/// Loading a project never changes the caller's registry, so the loader reads with this copy.
/// `devices` is the file's `devices` array (any depth is searched), `path` its JSON path.
pub(crate) fn stand_in_registry(
    devices: &Value,
    path: &JsonPath,
    registry: &Registry,
) -> Result<Option<Registry>, ModelError> {
    let mut missing: Vec<(String, Header)> = Vec::new();
    collect_missing(devices, path, registry, 0, &mut missing)?;
    if missing.is_empty() {
        return Ok(None);
    }
    let mut copy = Registry::new();
    let fail = |e: atomik_device_sdk::RegistryError| ModelError::Patch(format!("$: {e}"));
    for t in registry.type_ids() {
        let factory = *registry.get(t).expect("listed types are registered");
        if registry.takes_samples(t) {
            copy.register_with_samples(factory).map_err(fail)?;
        } else if registry.takes_player_data(t) {
            copy.register_with_player_data(factory).map_err(fail)?;
        } else {
            copy.register(factory).map_err(fail)?;
        }
    }
    for t in registry.plugin_type_ids() {
        let desc = registry.descriptor(t).expect("listed types are registered");
        copy.register_plugin(t.to_owned(), desc).map_err(fail)?;
    }
    for (type_id, h) in missing {
        let desc = descriptor_for(&h.plugin, &h.layout)
            .map_err(|e| ModelError::Patch(format!("$: {type_id}: {e}")))?;
        copy.register_plugin(type_id, desc).map_err(fail)?;
    }
    Ok(Some(copy))
}

fn collect_missing(
    devices: &Value,
    path: &JsonPath,
    registry: &Registry,
    depth: usize,
    out: &mut Vec<(String, Header)>,
) -> Result<(), ModelError> {
    // Deeper nesting is reported by the container rules with its own path.
    if depth > crate::MAX_CONTAINER_DEPTH {
        return Ok(());
    }
    let Some(list) = devices.as_array() else {
        return Ok(());
    };
    for (i, d) in list.iter().enumerate() {
        let p = path.index(i);
        let Some(o) = d.as_object() else { continue };
        if let Some(type_id) = o.get("type").and_then(Value::as_str)
            && is_plugin_type_id(type_id)
            && registry.get(type_id).is_none()
            && let Some(plugin) = o.get("plugin")
        {
            let po = object(plugin, &p.field("plugin"), PLUGIN_FIELDS, PLUGIN_REQUIRED)?;
            let header = load_header(po, &p.field("plugin"), type_id)?;
            // Each instance saved only the rows it uses: the type gets the union.
            match out.iter_mut().find(|(t, _)| t == type_id) {
                Some((_, first)) => crate::plugin::merge_rows(&mut first.layout, &header.layout),
                None => out.push((type_id.to_owned(), header)),
            }
        }
        if let Some(inner) = o.get("inner").and_then(|v| v.get("devices")) {
            collect_missing(
                inner,
                &p.field("inner").field("devices"),
                registry,
                depth + 1,
                out,
            )?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------ writing

/// Writes `{ … }` (no leading newline or comma) for the plugin object of a device whose fields
/// are at indent `p`. `used` are the param ids the project uses; only layout rows with those
/// ids are written, sorted by id.
pub(crate) fn write_plugin(
    w: &mut String,
    plugin: &PluginDoc,
    p: &str,
    used: Option<&BTreeSet<u32>>,
) {
    let q = format!("{p}  ");
    let r = format!("{p}    ");
    let l = &plugin.layout;
    let _ = write!(
        w,
        "{{\n{q}\"format\": {}, \"id\": {},\n{q}\"name\": {}, \"vendor\": {}, \"version\": {}",
        json_str(plugin.format.name()),
        json_str(&plugin.id),
        json_str(&l.name),
        json_str(&l.vendor),
        json_str(&l.version),
    );
    if let Some(hint) = &plugin.path_hint {
        let _ = write!(w, ",\n{q}\"path_hint\": {}", json_str(hint));
    }
    let channel = |c: Option<Channels>| match channels_name(c) {
        Some(n) => json_str(n),
        None => "null".to_owned(),
    };
    let _ = write!(
        w,
        ",\n{q}\"layout\": {{\n{r}\"main_in\": {}, \"main_out\": {}, \"sidechain\": {}, \"notes_in\": {}, \"notes_out\": {}, \"kind\": {},\n{r}\"params\": [",
        channel(l.main_in),
        channel(l.main_out),
        l.sidechain,
        l.notes_in,
        l.notes_out,
        json_str(kind_name(l.kind_hint)),
    );
    let mut rows: Vec<&PluginParamInfo> = l
        .params
        .iter()
        .filter(|row| used.is_some_and(|u| u.contains(&row.id)))
        .collect();
    rows.sort_by_key(|row| row.id);
    for (i, row) in rows.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let module = if row.module.is_empty() {
            String::new()
        } else {
            format!(", \"module\": {}", json_str(&row.module))
        };
        let _ = write!(
            w,
            "{sep}\n{r}  {{\"id\": {}, \"name\": {}{module}, \"min\": {}, \"max\": {}, \"default\": {}, \"stepped\": {}, \"steps\": {}}}",
            row.id,
            json_str(&row.name),
            f64_text(row.min),
            f64_text(row.max),
            f64_text(row.default),
            row.stepped,
            row.steps,
        );
    }
    let _ = write!(
        w,
        "{}]\n{q}}}",
        if rows.is_empty() {
            String::new()
        } else {
            format!("\n{r}")
        }
    );
    if let Some(state) = &plugin.state {
        let _ = write!(w, ",\n{q}\"state\": \"{}\"", base64_encode(&state.0));
    }
    let pins: Vec<String> = plugin.pinned.iter().map(u32::to_string).collect();
    let _ = write!(
        w,
        ",\n{q}\"pinned\": [{}],\n{q}\"run_on_audio_thread\": {}, \"powered\": {}\n{p}}}",
        pins.join(", "),
        plugin.run_on_audio_thread,
        plugin.powered,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (plain, text) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), text);
            assert_eq!(base64_decode(text).unwrap(), plain.as_bytes());
            assert_eq!(base64_decoded_len(text), Some(plain.len() as u64));
        }
    }

    #[test]
    fn base64_round_trips_every_byte_value_and_length() {
        let all: Vec<u8> = (0..=255).collect();
        for len in 0..all.len() {
            let text = base64_encode(&all[..len]);
            assert_eq!(base64_decode(&text).unwrap(), &all[..len], "{len}");
        }
    }

    #[test]
    fn base64_rejects_everything_that_is_not_canonical() {
        for bad in [
            "Zg=",       // length
            "Zg",        // unpadded
            "Zm9v\n",    // line break
            "Zm9v Zg==", // space
            "Zg==Zm9v",  // padding in the middle
            "Zh==",      // non-zero bits behind the data
            "Zm9=",      // non-zero bits behind the data
            "=Zg=",      // padding first
            "Z===",      // three paddings
            "Zm-v",      // url-safe alphabet
            "Zm9\u{e9}", // not ASCII
        ] {
            assert!(base64_decode(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn the_decoded_length_needs_no_decoding() {
        assert_eq!(base64_decoded_len("Zm9vYg=="), Some(4));
        assert_eq!(base64_decoded_len("Zm9vYmE="), Some(5));
        assert_eq!(base64_decoded_len("Zm9v"), Some(3));
        assert_eq!(base64_decoded_len("Z==="), None);
        assert_eq!(base64_decoded_len("Zm9"), None);
    }
}
