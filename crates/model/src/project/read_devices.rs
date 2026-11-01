//! Reading a device's sample zones and step, curve and drum pattern data, and the asset pool.

use super::*;

/// Reads a device's `"zones"` array (shape only; the pool and ranges are checked by
/// [`check_device_zones`] once the pool is loaded).
pub(crate) fn load_zones(
    v: &Value,
    path: &JsonPath,
    file_version: u64,
) -> Result<Vec<ZoneDoc>, ModelError> {
    // Version 6 added `reverse`, version 39 the optional group, crossfade, round-robin and
    // per-zone envelope and filter fields.
    let version = file_version;
    let with_reverse = version >= 6;
    let mut zones = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let z = schema::object(item, &p, Obj::Zone, version)?;
        let u = |name: &str, max: u64| uint(&z[name], &p.field(name), max);
        let f = |name: &str| number(&z[name], &p.field(name)).map(|x| x as f32);
        let transpose_path = p.field("transpose");
        let transpose = int(&z["transpose"], &transpose_path)?;
        let transpose = i8::try_from(transpose)
            .map_err(|_| transpose_path.error(format!("{transpose} is outside -128..=127")))?;
        let loop_mode = match string(&z["loop_mode"], &p.field("loop_mode"))? {
            "off" => LoopMode::Off,
            "forward" => LoopMode::Forward,
            "ping_pong" => LoopMode::PingPong,
            other => {
                return Err(p.field("loop_mode").error(format!(
                    "expected \"off\", \"forward\" or \"ping_pong\", found {other:?}"
                )));
            }
        };
        let trigger = match string(&z["trigger"], &p.field("trigger"))? {
            "gate" => TriggerMode::Gate,
            "one_shot" => TriggerMode::OneShot,
            other => {
                return Err(p.field("trigger").error(format!(
                    "expected \"gate\" or \"one_shot\", found {other:?}"
                )));
            }
        };
        zones.push(ZoneDoc {
            asset: string(&z["asset"], &p.field("asset"))?.to_owned(),
            key_lo: u("key_lo", 255)? as u8,
            key_hi: u("key_hi", 255)? as u8,
            vel_lo: u("vel_lo", 255)? as u8,
            vel_hi: u("vel_hi", 255)? as u8,
            root_key: u("root_key", 255)? as u8,
            transpose,
            tune_cents: f("tune_cents")?,
            gain_db: f("gain_db")?,
            pan: f("pan")?,
            start: u("start", MAX_AUDIO_FRAMES)?,
            end: u("end", MAX_AUDIO_FRAMES)?,
            loop_mode,
            loop_start: u("loop_start", MAX_AUDIO_FRAMES)?,
            loop_end: u("loop_end", MAX_AUDIO_FRAMES)?,
            crossfade: u("crossfade", MAX_AUDIO_FRAMES)?,
            trigger,
            reverse: if with_reverse {
                boolean(&z["reverse"], &p.field("reverse"))?
            } else {
                false
            },
            ext: load_zone_ext(&z, &p)?,
        });
    }
    Ok(zones)
}

/// The optional zone fields of format 39: each is absent when it has its default, and the
/// envelope and filter fields come as a set.
fn load_zone_ext(
    z: &serde_json::Map<String, Value>,
    p: &JsonPath,
) -> Result<atomik_device_sdk::ZoneExt, ModelError> {
    use atomik_device_sdk::{ZoneEnv, ZoneExt, ZoneFilter};
    let small = |name: &str| -> Result<u8, ModelError> {
        z.get(name)
            .map_or(Ok(0), |v| uint(v, &p.field(name), 255).map(|n| n as u8))
    };
    let num = |name: &str| -> Result<f32, ModelError> {
        let v = z.get(name).ok_or_else(|| {
            p.error(format!(
                "`{name}` is missing (the group of fields needs it)"
            ))
        })?;
        number(v, &p.field(name)).map(|x| x as f32)
    };
    let env = if z.contains_key("env_attack") {
        Some(ZoneEnv {
            attack_ms: num("env_attack")?,
            decay_ms: num("env_decay")?,
            sustain: num("env_sustain")?,
            release_ms: num("env_release")?,
        })
    } else {
        None
    };
    let filter = if z.contains_key("flt_cutoff") {
        let mode = z.get("flt_mode").map_or(Ok(0), |v| {
            uint(v, &p.field("flt_mode"), 255).map(|n| n as u8)
        })?;
        Some(ZoneFilter {
            mode,
            cutoff: num("flt_cutoff")?,
            resonance: num("flt_resonance")?,
            key_track: num("flt_key_track")?,
            env_amount: num("flt_env_amount")?,
        })
    } else {
        None
    };
    Ok(ZoneExt {
        group: small("group")?,
        vel_xfade: small("vel_xfade")?,
        rr_len: small("rr_len")?,
        rr_pos: small("rr_pos")?,
        env,
        filter,
    })
}

/// An unsigned integer in `lo..=hi`.
pub(super) fn ranged(v: &Value, path: &JsonPath, lo: u64, hi: u64) -> Result<u8, ModelError> {
    let n = uint(v, path, u64::MAX)?;
    if !(lo..=hi).contains(&n) {
        return Err(path.error(format!("{n} is outside {lo}..={hi}")));
    }
    Ok(n as u8)
}

/// Reads a device's `"steps"` object (v7). Whether the device may have one is checked by the
/// caller, which knows the registry.
pub(crate) fn load_steps(v: &Value, path: &JsonPath) -> Result<StepSeqDoc, ModelError> {
    let o = schema::current(v, path, Obj::Patterns)?;
    let list_path = path.field("patterns");
    let list = array(&o["patterns"], &list_path)?;
    if !(1..=MAX_PATTERNS).contains(&list.len()) {
        return Err(list_path.error(format!(
            "{} patterns is outside 1..={MAX_PATTERNS}",
            list.len()
        )));
    }
    let mut patterns = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let p = list_path.index(i);
        let o = schema::current(item, &p, Obj::Pattern)?;
        let name_path = p.field("name");
        let name = string(&o["name"], &name_path)?;
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(name_path.error(format!("longer than {MAX_NAME_CHARS} characters")));
        }
        let resolution_path = p.field("resolution");
        let resolution_name = string(&o["resolution"], &resolution_path)?;
        let resolution = SeqResolution::from_name(resolution_name).ok_or_else(|| {
            resolution_path.error(format!(
                "unknown resolution {resolution_name:?}; expected one of: {}",
                SeqResolution::ALL.map(SeqResolution::name).join(", ")
            ))
        })?;
        let length = ranged(&o["length"], &p.field("length"), 1, MAX_STEPS as u64)?;
        let swing = ranged(&o["swing"], &p.field("swing"), 0, 75)?;
        let steps_path = p.field("steps");
        let raw = array(&o["steps"], &steps_path)?;
        if raw.len() != MAX_STEPS {
            return Err(steps_path.error(format!(
                "{} entries, exactly {MAX_STEPS} are required",
                raw.len()
            )));
        }
        let mut steps = Vec::with_capacity(MAX_STEPS);
        for (j, item) in raw.iter().enumerate() {
            let sp = steps_path.index(j);
            let s = schema::current(item, &sp, Obj::Step)?;
            steps.push(StepDoc {
                on: boolean(&s["on"], &sp.field("on"))?,
                key: ranged(&s["key"], &sp.field("key"), 0, 127)?,
                velocity: ranged(&s["velocity"], &sp.field("velocity"), 1, 127)?,
                gate: ranged(&s["gate"], &sp.field("gate"), 1, 100)?,
                prob: ranged(&s["prob"], &sp.field("prob"), 0, 100)?,
                tie: boolean(&s["tie"], &sp.field("tie"))?,
            });
        }
        patterns.push(PatternDoc {
            name: name.to_owned(),
            length,
            resolution,
            swing,
            steps,
        });
    }
    Ok(StepSeqDoc { patterns })
}

/// Reads a device's `"curves"` object (v32). Whether the device may have one is checked by the
/// caller, which knows the registry.
pub(crate) fn load_curves(v: &Value, path: &JsonPath) -> Result<CurveDoc, ModelError> {
    let o = schema::current(v, path, Obj::Patterns)?;
    let list_path = path.field("patterns");
    let list = array(&o["patterns"], &list_path)?;
    if !(1..=MAX_PATTERNS).contains(&list.len()) {
        return Err(list_path.error(format!(
            "{} patterns is outside 1..={MAX_PATTERNS}",
            list.len()
        )));
    }
    let mut patterns = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let p = list_path.index(i);
        let o = schema::current(item, &p, Obj::CurvePattern)?;
        let mut rows = [Vec::new(), Vec::new()];
        for (row, name) in rows
            .iter_mut()
            .zip(schema::current_fields(Obj::CurvePattern))
        {
            let row_path = p.field(name);
            let raw = array(&o[*name], &row_path)?;
            if raw.len() != MAX_STEPS {
                return Err(row_path.error(format!(
                    "{} entries, exactly {MAX_STEPS} are required",
                    raw.len()
                )));
            }
            for (j, item) in raw.iter().enumerate() {
                let sp = row_path.index(j);
                let s = schema::current(item, &sp, Obj::CurveStep)?;
                let value_path = sp.field("value");
                let value = s["value"].as_i64().ok_or_else(|| {
                    value_path.error(format!("expected an integer, found {}", s["value"]))
                })?;
                let max = i64::from(CURVE_VALUE_MAX);
                if !(-max..=max).contains(&value) {
                    return Err(value_path.error(format!("{value} is outside -{max}..={max}")));
                }
                row.push(CurveStepDoc {
                    value: value as i8,
                    slide: boolean(&s["slide"], &sp.field("slide"))?,
                });
            }
        }
        patterns.push(CurvePatternDoc { rows });
    }
    Ok(CurveDoc { patterns })
}

/// Reads a device's `"drums"` object (v33). Whether the device may have one is checked by the
/// caller, which knows the registry.
pub(crate) fn load_drums(v: &Value, path: &JsonPath) -> Result<DrumDoc, ModelError> {
    let o = schema::current(v, path, Obj::Patterns)?;
    let list_path = path.field("patterns");
    let list = array(&o["patterns"], &list_path)?;
    if !(1..=MAX_PATTERNS).contains(&list.len()) {
        return Err(list_path.error(format!(
            "{} patterns is outside 1..={MAX_PATTERNS}",
            list.len()
        )));
    }
    let mut patterns = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let p = list_path.index(i);
        let o = schema::current(item, &p, Obj::DrumPattern)?;
        let name_path = p.field("name");
        let name = string(&o["name"], &name_path)?;
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(name_path.error(format!("longer than {MAX_NAME_CHARS} characters")));
        }
        let resolution_path = p.field("resolution");
        let resolution_name = string(&o["resolution"], &resolution_path)?;
        let resolution = SeqResolution::from_name(resolution_name).ok_or_else(|| {
            resolution_path.error(format!(
                "unknown resolution {resolution_name:?}; expected one of: {}",
                SeqResolution::ALL.map(SeqResolution::name).join(", ")
            ))
        })?;
        let length = ranged(&o["length"], &p.field("length"), 1, MAX_STEPS as u64)?;
        let swing = ranged(&o["swing"], &p.field("swing"), 0, 75)?;
        let lanes_path = p.field("lanes");
        let raw = array(&o["lanes"], &lanes_path)?;
        if raw.len() != DRUM_LANES {
            return Err(lanes_path.error(format!(
                "{} entries, exactly {DRUM_LANES} are required",
                raw.len()
            )));
        }
        let mut lanes = Vec::with_capacity(DRUM_LANES);
        let mut steps = Vec::with_capacity(DRUM_LANES);
        for (j, item) in raw.iter().enumerate() {
            let lp = lanes_path.index(j);
            let l = schema::current(item, &lp, Obj::DrumLane)?;
            let role_path = lp.field("role");
            let role_name = string(&l["role"], &role_path)?;
            let role = DrumRole::from_name(role_name).ok_or_else(|| {
                role_path.error(format!(
                    "unknown role {role_name:?}; expected one of: {}",
                    DrumRole::ALL.map(DrumRole::name).join(", ")
                ))
            })?;
            lanes.push(DrumLaneDoc {
                key: ranged(&l["key"], &lp.field("key"), 0, 127)?,
                role,
                mute: boolean(&l["mute"], &lp.field("mute"))?,
            });
            let vel_path = lp.field("velocity");
            let prob_path = lp.field("prob");
            let vel = array(&l["velocity"], &vel_path)?;
            let prob = array(&l["prob"], &prob_path)?;
            for (what, raw, at) in [("velocity", vel, &vel_path), ("prob", prob, &prob_path)] {
                if raw.len() != MAX_STEPS {
                    return Err(at.error(format!(
                        "{} entries, exactly {MAX_STEPS} are required ({what})",
                        raw.len()
                    )));
                }
            }
            let mut row = Vec::with_capacity(MAX_STEPS);
            for k in 0..MAX_STEPS {
                row.push(DrumStepDoc {
                    velocity: ranged(&vel[k], &vel_path.index(k), 0, 127)?,
                    prob: ranged(&prob[k], &prob_path.index(k), 0, 100)?,
                });
            }
            steps.push(row);
        }
        patterns.push(DrumPatternDoc {
            name: name.to_owned(),
            length,
            resolution,
            swing,
            lanes,
            steps,
        });
    }
    Ok(DrumDoc { patterns })
}

/// Validates every device's zones (inside containers too) against the loaded pool and the
/// registry, as `Command::SetZones` does.
pub(super) fn check_device_zones(
    doc: &RackDoc,
    path: &JsonPath,
    registry: &Registry,
) -> Result<(), ModelError> {
    check_zones_in(&doc.devices, doc, path, registry)
}

pub(super) fn check_zones_in(
    devices: &[crate::RackDevice],
    doc: &RackDoc,
    path: &JsonPath,
    registry: &Registry,
) -> Result<(), ModelError> {
    for (i, d) in devices.iter().enumerate() {
        if !d.zones.is_empty() {
            let p = path.index(i).field("zones");
            if !registry.takes_samples(&d.type_id) {
                return Err(p.error(format!("{} does not take samples", d.type_id)));
            }
            crate::zones::check_zones(&doc.assets, &d.zones)
                .map_err(|(j, m)| p.index(j).error(m))?;
        }
        if let Some(inner) = &d.inner {
            let p = path.index(i).field("inner").field("devices");
            check_zones_in(&inner.devices, doc, &p, registry)?;
        }
    }
    Ok(())
}

/// Reads the sample pool: asset ids (64 lowercase hex digits) with their audio format.
pub(super) fn load_assets(
    v: &Value,
    path: &JsonPath,
) -> Result<BTreeMap<String, AssetMeta>, ModelError> {
    let map = v
        .as_object()
        .ok_or_else(|| path.error(format!("expected an object, found {}", describe(v))))?;
    let mut assets = BTreeMap::new();
    for (hash, item) in map {
        let p = path.field(hash);
        if !is_valid_asset_hash(hash) {
            return Err(p.error("asset ids are 64 lowercase hex digits"));
        }
        let a = schema::current(item, &p, Obj::Asset)?;
        let channels = uint(&a["channels"], &p.field("channels"), u64::from(u16::MAX))? as u16;
        if channels == 0 {
            return Err(p.field("channels").error("an asset has at least 1 channel"));
        }
        let sample_rate = uint(
            &a["sample_rate"],
            &p.field("sample_rate"),
            u64::from(u32::MAX),
        )? as u32;
        if sample_rate == 0 {
            return Err(p
                .field("sample_rate")
                .error("the sample rate must be above 0"));
        }
        let frames = uint(&a["frames"], &p.field("frames"), MAX_AUDIO_FRAMES)?;
        if frames == 0 {
            return Err(p.field("frames").error("an asset has at least 1 frame"));
        }
        let original_name = match a.get("name") {
            Some(n) => string(n, &p.field("name"))?.to_owned(),
            None => String::new(),
        };
        assets.insert(
            hash.clone(),
            AssetMeta {
                channels,
                sample_rate,
                frames,
                original_name,
            },
        );
    }
    Ok(assets)
}
