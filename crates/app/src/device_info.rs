//! Help > Export Device Parameter List: one text table of every parameter of every built-in
//! device, for scripts, mapping sheets and documentation.
//!
//! One line per parameter, in the order of the registry (type ids sorted) and of each device's
//! parameter table, so two exports of the same build are identical. The first line names the
//! columns: `device | key | name | unit | min | max | default | discrete`.

use atomik_core::Unit;
use atomik_device_sdk::Registry;
use std::path::Path;

/// The columns, as the first line of the file.
pub const HEADER: &str = "device | key | name | unit | min | max | default | discrete";

/// The text of a unit: its name, or the choices of a choice parameter joined with `/`.
fn unit_text(unit: Unit) -> String {
    match unit {
        Unit::None => "none".to_owned(),
        Unit::Hz => "Hz".to_owned(),
        Unit::Ms => "ms".to_owned(),
        Unit::Db => "dB".to_owned(),
        Unit::Percent => "%".to_owned(),
        Unit::Semitones => "semitones".to_owned(),
        Unit::Cents => "cents".to_owned(),
        Unit::Octaves => "octaves".to_owned(),
        Unit::Toggle => "toggle".to_owned(),
        Unit::Choice(names) => format!("choice({})", names.join("/")),
    }
}

/// The parameter table of every registered device.
pub fn device_info_text(registry: &Registry) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for type_id in registry.type_ids() {
        let Some(entry) = registry.get(type_id) else {
            continue;
        };
        for p in entry.descriptor.params {
            out.push_str(&format!(
                "{type_id} | {} | {} | {} | {} | {} | {} | {}\n",
                p.key,
                p.name,
                unit_text(p.unit),
                p.min,
                p.max,
                p.default,
                p.discrete
            ));
        }
    }
    out
}

/// The file name the save dialog proposes.
pub fn default_file_name() -> &'static str {
    "atomik-device-parameters.txt"
}

/// Writes [`device_info_text`] to `path`.
pub fn export(registry: &Registry, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, device_info_text(registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_devices() -> Registry {
        let mut r = Registry::new();
        atomik_devices::register_all(&mut r).unwrap();
        r
    }

    #[test]
    fn the_table_matches_every_descriptor_in_order() {
        let registry = all_devices();
        let text = device_info_text(&registry);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(HEADER));
        for type_id in registry.type_ids() {
            let d = registry.get(type_id).unwrap().descriptor;
            for p in d.params {
                let line = lines.next().expect("a line per parameter");
                let cols: Vec<&str> = line.split(" | ").collect();
                assert_eq!(cols.len(), 8, "{line}");
                assert_eq!(
                    (cols[0], cols[1], cols[2]),
                    (type_id, p.key, p.name),
                    "{line}"
                );
                assert_eq!(cols[4].parse::<f32>().unwrap(), p.min, "{line}");
                assert_eq!(cols[5].parse::<f32>().unwrap(), p.max, "{line}");
                assert_eq!(cols[6].parse::<f32>().unwrap(), p.default, "{line}");
                assert_eq!(cols[7], p.discrete.to_string(), "{line}");
            }
        }
        assert_eq!(lines.next(), None);
        assert_eq!(text, device_info_text(&all_devices()), "deterministic");
    }
}
