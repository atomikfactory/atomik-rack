//! The CPU readout on the name plates: the share of the audio time each device takes, set by
//! the app (View > CPU Meters). The figures are kept apart from the cached panel lists and
//! painted over them, so a changing number never rebuilds a panel.

use super::RackView;
use atomik_model::DeviceKey;
use std::collections::BTreeMap;

/// The load of `key` in `loads` (flat device keys, percent): its own plus every device inside it
/// (`key/...`, which is how a container sums its contents); `None` when neither has a reading.
pub fn load_under(loads: &BTreeMap<DeviceKey, f32>, key: &DeviceKey) -> Option<f32> {
    let inside = format!("{}/", key.0);
    let mut sum = None;
    for (k, v) in loads.range(key.clone()..) {
        if *k != *key && !k.0.starts_with(&inside) {
            // Keys sort by bytes, so `key/..` is not contiguous with `key` (`key-2` sorts in
            // between): keep scanning while the prefix could still match.
            if !k.0.starts_with(&key.0) {
                break;
            }
            continue;
        }
        sum = Some(sum.unwrap_or(0.0) + v);
    }
    sum
}

impl RackView {
    /// Shows `loads` (flat device keys, percent of the audio time, see [`load_under`]) on the
    /// name plates of the devices in view, a container with the sum of its contents. `None`
    /// hides the readout.
    pub fn set_cpu_loads(&mut self, loads: Option<&BTreeMap<DeviceKey, f32>>) {
        let scope = self.scope_path();
        let mut shown = BTreeMap::new();
        if let Some(loads) = loads {
            for d in &self.devices {
                let full = match &scope {
                    Some(p) => DeviceKey(format!("{}/{}", p.0, d.key.0)),
                    None => d.key.clone(),
                };
                if let Some(pct) = load_under(loads, &full) {
                    shown.insert(d.key.clone(), pct);
                }
            }
        }
        // Whole percents are all a plate shows: repaint only when one changed.
        let rounded = |m: &BTreeMap<DeviceKey, f32>| -> Vec<(DeviceKey, i32)> {
            m.iter()
                .map(|(k, v)| (k.clone(), v.round() as i32))
                .collect()
        };
        if rounded(&shown) != rounded(&self.cpu) {
            self.repaint = true;
        }
        self.cpu = shown;
    }

    /// The load shown on the plate of device `key`, if any.
    pub fn cpu_load(&self, key: &DeviceKey) -> Option<f32> {
        self.cpu.get(&self.local_key(key)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    #[test]
    fn a_container_shows_the_sum_of_what_is_inside() {
        let mut loads = BTreeMap::new();
        loads.insert(key("synth"), 4.0);
        loads.insert(key("group/a"), 1.5);
        loads.insert(key("group/inner/b"), 2.5);
        loads.insert(key("group-2/c"), 40.0);
        loads.insert(key("group2"), 30.0);
        assert_eq!(load_under(&loads, &key("synth")), Some(4.0));
        assert_eq!(load_under(&loads, &key("group")), Some(4.0));
        assert_eq!(load_under(&loads, &key("group/inner")), Some(2.5));
        assert_eq!(load_under(&loads, &key("group-2")), Some(40.0));
        assert_eq!(load_under(&loads, &key("nothing")), None);
    }

    #[test]
    fn a_device_and_the_devices_inside_it_add_up() {
        let mut loads = BTreeMap::new();
        loads.insert(key("group"), 1.0);
        loads.insert(key("group/a"), 2.0);
        assert_eq!(load_under(&loads, &key("group")), Some(3.0));
    }
}
