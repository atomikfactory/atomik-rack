//! The modulation matrix in the app core: what the matrix panel shows for a device.
//!
//! The slots are document data of the device ([`atomik_model::RackDevice::mods`]) and its
//! `mod<n>_amount` params; the panel edits them with ordinary commands (`SetModSlot`,
//! `SetParam`), so nothing here changes the document. This module only reads the tables the
//! registry holds for the device's type and the slots the device has, as the rows the panel
//! draws.

use super::*;
use atomik_device_sdk::MOD_SLOTS;
use atomik_ui::ModMatrixInfo;

impl<S: EngineSink> AppCore<S> {
    /// Whether `device` declares modulation tables (its type was registered with a matrix).
    pub fn has_mod_matrix(&self, device: &DeviceKey) -> bool {
        self.document
            .doc()
            .resolve(device)
            .is_some_and(|d| self.registry.mod_tables(&d.type_id).is_some())
    }

    /// The matrix of `device` as the panel shows it, or `None` when the device has none.
    pub fn mod_matrix_info(&self, device: &DeviceKey) -> Option<ModMatrixInfo> {
        let d = self.document.doc().resolve(device)?;
        let tables = self.registry.mod_tables(&d.type_id)?;
        Some(ModMatrixInfo {
            device: device.clone(),
            name: d.name.clone(),
            sources: tables.sources.iter().map(|s| s.name.to_owned()).collect(),
            targets: tables.targets.iter().map(|t| t.name.to_owned()).collect(),
            slots: (0..MOD_SLOTS).map(|n| d.mod_slot(n)).collect(),
            amounts: (0..MOD_SLOTS).map(|n| d.mod_amount(n)).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::RecordingSink;
    use atomik_model::{ModSlot, WorkflowCommand, empty_doc};

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    fn core_with(type_id: &str) -> AppCore<RecordingSink> {
        let mut c =
            AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.to_string(),
            key: Some(key("synth")),
        })
        .unwrap();
        c
    }

    #[test]
    fn a_device_with_tables_lists_them_and_its_slots() {
        let mut c = core_with("atomik.poly-synth");
        assert!(c.has_mod_matrix(&key("synth")));
        let info = c.mod_matrix_info(&key("synth")).expect("a matrix");
        assert!(!info.sources.is_empty() && !info.targets.is_empty());
        assert_eq!(info.slots, vec![None; MOD_SLOTS]);
        c.apply_command(Command::Workflow(WorkflowCommand::SetModSlot {
            device: key("synth"),
            slot: 1,
            value: Some(ModSlot::new(0, 0)),
        }))
        .unwrap();
        c.apply_command(Command::SetParam {
            key: key("synth"),
            param: "mod2_amount".into(),
            value: 0.25,
            merge: None,
        })
        .unwrap();
        let info = c.mod_matrix_info(&key("synth")).unwrap();
        assert_eq!(info.slots[1], Some(ModSlot::new(0, 0)));
        assert_eq!(info.amounts[1], 0.25);
    }

    #[test]
    fn a_device_without_tables_has_no_matrix() {
        let c = core_with("atomik.echo");
        assert!(!c.has_mod_matrix(&key("synth")));
        assert!(c.mod_matrix_info(&key("synth")).is_none());
    }
}
