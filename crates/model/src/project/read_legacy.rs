//! Reading files from before the console and the reserved `boundary` key: the version 1 migration and the rename.

use super::*;

/// Version 1 → 2: when the registry has the console devices, every
/// instrument gets a channel, in rack order, exactly as `ConsoleCommand::AddChannel` wires
/// it (so the master is created with the first one). Instruments that cannot get one (no
/// audio output) are skipped. Without console devices the console stays empty.
pub(super) fn migrate_v1(doc: RackDoc, registry: &Registry) -> RackDoc {
    if !crate::console_cmd::console_available(registry) {
        return doc;
    }
    let instruments: Vec<DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| crate::console_cmd::is_instrument(registry, &d.type_id))
        .map(|d| d.key.clone())
        .collect();
    let mut document = crate::Document::new(doc);
    for source in instruments {
        // A rejected channel leaves the document unchanged.
        let _ = document.apply(
            crate::Command::Console(crate::ConsoleCommand::AddChannel { source }),
            registry,
        );
    }
    document.doc().clone()
}

/// A device keyed `boundary` in a file older than version 8: the key is reserved for a
/// container's inside edge, so the device becomes `boundary-1` (or the first free
/// `boundary-N`) and every reference to it follows: cables, console channels, sources, master
/// and returns, song tracks. Returns whether anything was renamed; the warning says what.
pub(super) fn rename_boundary(value: &mut Value, warnings: &mut Vec<String>) -> bool {
    let key_of = |d: &Value| d.get("id").and_then(Value::as_str).map(str::to_owned);
    let Some(devices) = value.get("devices").and_then(Value::as_array) else {
        return false;
    };
    let ids: Vec<String> = devices.iter().filter_map(key_of).collect();
    if !ids.iter().any(|k| k == crate::BOUNDARY_KEY) {
        return false;
    }
    let new = (1u32..)
        .map(|n| format!("boundary-{n}"))
        .find(|k| !ids.contains(k))
        .expect("a free key exists");
    let rename = |v: &mut Value| {
        if v.as_str() == Some(crate::BOUNDARY_KEY) {
            *v = Value::from(new.as_str());
        }
    };
    let rename_end = |v: &mut Value| {
        if let Some(text) = v.as_str()
            && let Some(socket) = text.strip_prefix("boundary:")
        {
            *v = Value::from(format!("{new}:{socket}"));
        }
    };
    if let Some(list) = value.get_mut("devices").and_then(Value::as_array_mut) {
        for d in list {
            if let Some(id) = d.get_mut("id") {
                rename(id);
            }
        }
    }
    if let Some(list) = value.get_mut("cables").and_then(Value::as_array_mut) {
        for c in list {
            for end in ["from", "to"] {
                if let Some(v) = c.get_mut(end) {
                    rename_end(v);
                }
            }
        }
    }
    if let Some(console) = value.get_mut("console") {
        if let Some(list) = console.get_mut("channels").and_then(Value::as_array_mut) {
            for c in list {
                for field in ["channel", "source"] {
                    if let Some(v) = c.get_mut(field) {
                        rename(v);
                    }
                }
            }
        }
        if let Some(v) = console.get_mut("master") {
            rename(v);
        }
        if let Some(list) = console.get_mut("returns").and_then(Value::as_array_mut) {
            list.iter_mut().for_each(rename);
        }
    }
    if let Some(list) = value
        .get_mut("song")
        .and_then(|s| s.get_mut("tracks"))
        .and_then(Value::as_array_mut)
    {
        for t in list {
            if let Some(v) = t.get_mut("device") {
                rename(v);
            }
        }
    }
    warnings.push(format!(
        "the device \"boundary\" was renamed \"{new}\": the key is reserved for the inside edge of a container"
    ));
    true
}
