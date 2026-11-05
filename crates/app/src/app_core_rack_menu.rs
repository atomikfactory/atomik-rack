//! What the rack's Create and Replace Device menus and the drop of a device row on a track
//! header ask of the app core: the device types a menu lists, creating a device of a chosen
//! type as the browser does, and replacing a device in place by one of the same family.

use super::*;
use crate::context_menus::DeviceMenus;
use atomik_model::{CreateSource, replace_reason};
use atomik_ui::browser::{device_groups, is_offered};

impl<S: EngineSink> AppCore<S> {
    /// The device types the rack menus list: every type that can be created at the top of the
    /// rack, and the types the selected device could be replaced by (those the model accepts
    /// for it; none when several devices or none are selected).
    pub fn device_menus(&self) -> DeviceMenus {
        let create = device_groups(&self.registry, |_| true);
        let replace = match self.replace_target() {
            Some(target) => {
                let current = self.doc().device(&target).map(|d| d.type_id.clone());
                device_groups(&self.registry, |d| {
                    current.as_deref() != Some(d.type_id)
                        && is_offered(d, false, 0)
                        && replace_reason(
                            self.doc(),
                            &self.registry,
                            &target,
                            &CreateSource::Type(d.type_id.to_string()),
                        )
                        .is_none()
                })
            }
            None => Vec::new(),
        };
        DeviceMenus { create, replace }
    }

    /// The device the Replace Device menu works on: the primary one of a rack selection of
    /// exactly one device.
    fn replace_target(&self) -> Option<DeviceKey> {
        if self.view.selection_len() != 1 {
            return None;
        }
        self.view.primary()
    }

    /// Creates a device of the built-in type `type_id` as picking its row in the device browser
    /// does (an instrument gets its track and channel, an effect goes after the selected
    /// device, a player into its stack). A refusal is the message.
    pub fn create_device_of_type(&mut self, type_id: &str) {
        let Some(desc) = self.registry.descriptor(type_id) else {
            self.report(format!("There is no device type '{type_id}'"));
            return;
        };
        let action =
            self.view
                .add_device_action_for(desc, self.document.doc(), atomik_ui::Modifiers::NONE);
        self.handle_action(action);
    }

    /// Replaces the selected device by a device of the built-in type `type_id`: its track,
    /// channel, cables and place stay, and so do the values of the params both have. One undo
    /// step; the model's refusal is the message.
    pub fn replace_selected_device(&mut self, type_id: &str) {
        let Some(target) = self.replace_target() else {
            self.report("Select one device to replace");
            return;
        };
        self.replace_device_with(target, CreateSource::Type(type_id.to_string()));
    }

    /// Replaces `target` by a device of `source` as one undo step. The model's refusal is
    /// reported by the command, as for every edit.
    pub fn replace_device_with(&mut self, target: DeviceKey, source: CreateSource) {
        let _ = self.apply_command(Command::Workflow(WorkflowCommand::ReplaceDevice {
            target,
            source,
        }));
    }

    /// A row of the device browser dragged over the arrangement at window point `pos`: the
    /// device of the track header under it, outlined, when `source` may replace it. `None` for
    /// `pos` or `source` clears the outline.
    pub fn header_replace_hover(
        &mut self,
        pos: Option<Point>,
        source: Option<&CreateSource>,
    ) -> Option<DeviceKey> {
        self.arrangement
            .replace_hover(pos, source, self.document.doc(), &self.registry)
    }
}
