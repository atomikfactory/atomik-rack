//! Reading the console: channels, buses, VCA groups and mixer snapshots.

use super::*;

/// Reads the console's VCA groups (version 26): ids from 1 and unique, a trimmed name of at
/// most 32 characters unique ignoring case, an optional colour, a `level_db` within the VCA
/// range (default 0), optional mute and solo flags, members that are console channels or buses,
/// none in two groups (listed in any order, kept in console order). At most 64 groups.
pub(super) fn load_vcas(
    v: &Value,
    path: &JsonPath,
    console: &mut Console,
) -> Result<(), ModelError> {
    let items = array(v, path)?;
    if items.len() > MAX_VCAS {
        return Err(path.error(format!(
            "at most {MAX_VCAS} VCA groups, found {}",
            items.len()
        )));
    }
    let mut taken: BTreeSet<DeviceKey> = BTreeSet::new();
    for (i, item) in items.iter().enumerate() {
        let p = path.index(i);
        let g = schema::current(item, &p, Obj::Vca)?;
        let id_path = p.field("id");
        let id = uint(&g["id"], &id_path, u64::from(u32::MAX))? as u32;
        if id == 0 {
            return Err(id_path.error("VCA ids count from 1"));
        }
        if console.vca(id).is_some() {
            return Err(id_path.error(format!("the VCA id {id} is used twice")));
        }
        let name_path = p.field("name");
        let name = string(&g["name"], &name_path)?;
        if name.trim() != name || name.is_empty() {
            return Err(name_path.error(format!(
                "{name:?} is not a valid VCA name: it must be non-empty with no space at either end"
            )));
        }
        if name.chars().count() > MAX_VCA_NAME_CHARS {
            return Err(name_path.error(format!(
                "a VCA name has at most {MAX_VCA_NAME_CHARS} characters"
            )));
        }
        if console.vca_by_name(name).is_some() {
            return Err(name_path.error(format!("the VCA name {name:?} is used twice")));
        }
        let color = match g.get("color") {
            Some(c) => parse_color(c, &p.field("color"))?,
            None => None,
        };
        let level_db = match g.get("level_db") {
            Some(l) => {
                let lp = p.field("level_db");
                let x = number(l, &lp)? as f32;
                if !(VCA_MIN_DB..=VCA_MAX_DB).contains(&x) {
                    return Err(lp.error(format!("{x} is outside {VCA_MIN_DB}..={VCA_MAX_DB}")));
                }
                x
            }
            None => 0.0,
        };
        let members_path = p.field("members");
        let mut members = Vec::new();
        for (j, m) in array(&g["members"], &members_path)?.iter().enumerate() {
            let mp = members_path.index(j);
            let key = DeviceKey(string(m, &mp)?.to_owned());
            if console.channel_index(&key).is_none() && console.bus_index(&key).is_none() {
                return Err(mp.error(format!("{:?} is not a console channel or bus", key.0)));
            }
            if !taken.insert(key.clone()) {
                return Err(mp.error(format!("{:?} is in more than one VCA group", key.0)));
            }
            members.push(key);
        }
        let order: Vec<&DeviceKey> = console.strip_keys().collect();
        members.sort_by_key(|m| order.iter().position(|k| *k == m).unwrap_or(usize::MAX));
        console.vcas.push(VcaGroup {
            id,
            name: name.to_owned(),
            color,
            level_db,
            mute: flag(g, &p, "mute")?,
            solo: flag(g, &p, "solo")?,
            members,
        });
    }
    Ok(())
}

/// Reads the console's mixer snapshots (version 27): ids from 1 and unique, a trimmed name of at
/// most 32 characters unique ignoring case, the params of each captured device (finite numbers)
/// and the groups as they were. The keys are not checked against the rack: a snapshot may name
/// what was deleted since. At most 16 snapshots.
pub(super) fn load_snapshots(
    v: &Value,
    path: &JsonPath,
    console: &mut Console,
) -> Result<(), ModelError> {
    let items = array(v, path)?;
    if items.len() > MAX_SNAPSHOTS {
        return Err(path.error(format!(
            "at most {MAX_SNAPSHOTS} snapshots, found {}",
            items.len()
        )));
    }
    for (i, item) in items.iter().enumerate() {
        let p = path.index(i);
        let s = schema::current(item, &p, Obj::Snapshot)?;
        let id_path = p.field("id");
        let id = uint(&s["id"], &id_path, u64::from(u32::MAX))? as u32;
        if id == 0 {
            return Err(id_path.error("snapshot ids count from 1"));
        }
        if console.snapshot(id).is_some() {
            return Err(id_path.error(format!("the snapshot id {id} is used twice")));
        }
        let name_path = p.field("name");
        let name = string(&s["name"], &name_path)?;
        if name.trim() != name || name.is_empty() {
            return Err(name_path.error(format!(
                "{name:?} is not a valid snapshot name: it must be non-empty with no space at either end"
            )));
        }
        if name.chars().count() > MAX_SNAPSHOT_NAME_CHARS {
            return Err(name_path.error(format!(
                "a snapshot name has at most {MAX_SNAPSHOT_NAME_CHARS} characters"
            )));
        }
        if console.snapshot_by_name(name).is_some() {
            return Err(name_path.error(format!("the snapshot name {name:?} is used twice")));
        }
        let mut state = MixerState::default();
        let devices_path = p.field("devices");
        let devices = s["devices"]
            .as_object()
            .ok_or_else(|| devices_path.error("expected an object from device key to params"))?;
        for (key, params) in devices {
            let dp = devices_path.field(key);
            let params = params
                .as_object()
                .ok_or_else(|| dp.error("expected an object from param key to value"))?;
            let mut stored = BTreeMap::new();
            for (param, value) in params {
                let vp = dp.field(param);
                let x = number(value, &vp)? as f32;
                if !x.is_finite() {
                    return Err(vp.error("expected a finite number"));
                }
                stored.insert(param.clone(), x);
            }
            state.devices.insert(DeviceKey(key.clone()), stored);
        }
        if let Some(vcas) = s.get("vcas") {
            let vcas_path = p.field("vcas");
            let mut seen = BTreeSet::new();
            for (j, item) in array(vcas, &vcas_path)?.iter().enumerate() {
                let gp = vcas_path.index(j);
                let g = schema::current(item, &gp, Obj::SnapshotVca)?;
                let gid_path = gp.field("id");
                let gid = uint(&g["id"], &gid_path, u64::from(u32::MAX))? as u32;
                if gid == 0 || !seen.insert(gid) {
                    return Err(gid_path.error(format!("the VCA id {gid} is 0 or used twice")));
                }
                let level_db = match g.get("level_db") {
                    Some(l) => {
                        let lp = gp.field("level_db");
                        let x = number(l, &lp)? as f32;
                        if !(VCA_MIN_DB..=VCA_MAX_DB).contains(&x) {
                            return Err(
                                lp.error(format!("{x} is outside {VCA_MIN_DB}..={VCA_MAX_DB}"))
                            );
                        }
                        x
                    }
                    None => 0.0,
                };
                let members_path = gp.field("members");
                let mut members = Vec::new();
                for (k, m) in array(&g["members"], &members_path)?.iter().enumerate() {
                    members.push(DeviceKey(string(m, &members_path.index(k))?.to_owned()));
                }
                state.vcas.push(SnapshotVca {
                    id: gid,
                    name: string(&g["name"], &gp.field("name"))?.to_owned(),
                    level_db,
                    mute: flag(g, &gp, "mute")?,
                    solo: flag(g, &gp, "solo")?,
                    members,
                });
            }
        }
        console.snapshots.push(MixerSnapshot {
            id,
            name: name.to_owned(),
            state,
        });
    }
    Ok(())
}

/// Reads and validates the console section against the loaded rack (the invariants
/// are listed in [`crate::console`]).
pub(super) fn load_console(
    v: &Value,
    path: &JsonPath,
    doc: &RackDoc,
    version: u64,
) -> Result<Console, ModelError> {
    let obj = schema::object(v, path, Obj::Console, version)?;
    // `null`, or the key of an existing device.
    let device_ref = |v: &Value, p: &JsonPath| -> Result<Option<DeviceKey>, ModelError> {
        if v.is_null() {
            return Ok(None);
        }
        let key = DeviceKey(string(v, p)?.to_owned());
        if doc.device(&key).is_none() {
            return Err(p.error(format!("no device has id {:?}", key.0)));
        }
        Ok(Some(key))
    };
    let type_of = |k: &DeviceKey| doc.device(k).map_or("", |d| d.type_id.as_str());
    let mut console = Console::default();

    let channels_path = path.field("channels");
    for (i, item) in array(&obj["channels"], &channels_path)?.iter().enumerate() {
        let p = channels_path.index(i);
        let ch = schema::current(item, &p, Obj::Channel)?;
        let ch_path = p.field("channel");
        let channel = device_ref(&ch["channel"], &ch_path)?
            .ok_or_else(|| ch_path.error("expected a device id, found null"))?;
        if type_of(&channel) != CHANNEL_TYPE {
            return Err(ch_path.error(format!(
                "{:?} is a {}, not an {CHANNEL_TYPE}",
                channel.0,
                type_of(&channel)
            )));
        }
        if console.channel_index(&channel).is_some() {
            return Err(ch_path.error(format!("{:?} is listed twice", channel.0)));
        }
        let src_path = p.field("source");
        let source = device_ref(&ch["source"], &src_path)?;
        if let Some(s) = &source
            && is_console_type(type_of(s))
        {
            return Err(src_path.error(format!(
                "{:?} is a console device; it cannot feed a channel",
                s.0
            )));
        }
        console.channels.push(ConsoleChannel { channel, source });
    }

    let master_path = path.field("master");
    console.master = device_ref(&obj["master"], &master_path)?;
    if let Some(m) = &console.master
        && type_of(m) != MASTER_TYPE
    {
        return Err(master_path.error(format!(
            "{:?} is a {}, not an {MASTER_TYPE}",
            m.0,
            type_of(m)
        )));
    }

    if let Some(buses) = obj.get("buses") {
        let buses_path = path.field("buses");
        for (i, item) in array(buses, &buses_path)?.iter().enumerate() {
            let p = buses_path.index(i);
            let b = schema::current(item, &p, Obj::Bus)?;
            let bus_path = p.field("bus");
            let bus = device_ref(&b["bus"], &bus_path)?
                .ok_or_else(|| bus_path.error("expected a device id, found null"))?;
            if type_of(&bus) != BUS_TYPE {
                return Err(bus_path.error(format!(
                    "{:?} is a {}, not an {BUS_TYPE}",
                    bus.0,
                    type_of(&bus)
                )));
            }
            if console.bus_index(&bus).is_some() {
                return Err(bus_path.error(format!("{:?} is listed twice", bus.0)));
            }
            if console.master.is_none() {
                return Err(bus_path.error("the console has no master, so it has no buses"));
            }
            let name_path = p.field("name");
            let name = string(&b["name"], &name_path)?;
            if name.trim() != name || name.is_empty() {
                return Err(name_path.error(format!(
                    "{name:?} is not a valid bus name: it must be non-empty with no space at either end"
                )));
            }
            if name.chars().count() > MAX_BUS_NAME_CHARS {
                return Err(name_path.error(format!(
                    "a bus name has at most {MAX_BUS_NAME_CHARS} characters"
                )));
            }
            if console.bus_by_name(name).is_some() {
                return Err(name_path.error(format!("the bus name {name:?} is used twice")));
            }
            console.buses.push(ConsoleBus {
                bus,
                name: name.to_owned(),
            });
        }
    }

    if let Some(vcas) = obj.get("vcas") {
        load_vcas(vcas, &path.field("vcas"), &mut console)?;
    }
    if let Some(snapshots) = obj.get("snapshots") {
        load_snapshots(snapshots, &path.field("snapshots"), &mut console)?;
    }

    let returns_path = path.field("returns");
    let returns = array(&obj["returns"], &returns_path)?;
    if returns.len() != FX_SLOTS {
        return Err(returns_path.error(format!(
            "expected {FX_SLOTS} return slots, found {}",
            returns.len()
        )));
    }
    for (slot, item) in returns.iter().enumerate() {
        let p = returns_path.index(slot);
        let Some(effect) = device_ref(item, &p)? else {
            continue;
        };
        if console.master.is_none() {
            return Err(p.error("the console has no master, so it has no returns"));
        }
        if is_console_type(type_of(&effect)) {
            return Err(p.error(format!(
                "{:?} is a console device, not a return effect",
                effect.0
            )));
        }
        if let Some(first) = console.return_slot(&effect) {
            return Err(p.error(format!("{:?} is already on return {}", effect.0, first + 1)));
        }
        if let Some(c) = console.channel_of(&effect) {
            return Err(p.error(format!(
                "{:?} feeds channel {:?}; it cannot also be a return effect",
                effect.0, c.channel.0
            )));
        }
        console.returns[slot] = Some(effect);
    }
    console.compensation = boolean(&obj["compensation"], &path.field("compensation"))?;
    if let Some(v) = obj.get("align_returns") {
        console.align_returns = boolean(v, &path.field("align_returns"))?;
    }
    Ok(console)
}
