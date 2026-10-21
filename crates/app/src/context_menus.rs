//! What the context menus list: per target, the registry commands
//! in order with their enabled flags. Labels and shortcut text are not written here: the menu
//! widget takes the label from the registry and the shortcut from the active keymap, so a key
//! and a menu item cannot differ.

use atomik_ui::ContextTarget;
use atomik_ui::context_menu::Spec;

/// What decides which items of a context menu can be chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuEnable {
    /// The pane has a selection Cut, Copy, Duplicate and Delete can work on.
    pub selection: bool,
    /// The clipboard holds something the pane can paste.
    pub paste: bool,
    /// Rack: the primary device has a song track (Go to Track).
    pub has_track: bool,
    /// Arrangement: two or more clips are selected (Join).
    pub join: bool,
    /// Arrangement: a selected clip is a note clip (Open in the editor).
    pub open_clip: bool,
    /// Arrangement: every selected clip is muted ("Unmute" instead of "Mute").
    pub clips_muted: bool,
    /// Piano roll: every selected note is muted ("Unmute Notes" instead of "Mute Notes").
    pub notes_muted: bool,
    /// Rack: the selection is the Master Section alone: it cannot be cut, copied, duplicated
    /// or deleted.
    pub master_only: bool,
    /// Rack: "Use as Send Effect" can work on the selection (the model would accept it).
    pub assign_send: bool,
    /// Rack: how many columns the rack has (Remove Column needs two, Add Column room).
    pub columns: u16,
}

fn item(command: &'static str, on: bool) -> Spec {
    Spec::item(command).enabled(on)
}

/// The items of the menu opened on `target`.
pub fn specs(target: ContextTarget, e: &MenuEnable) -> Vec<Spec> {
    let edit = |label: Option<(&'static str, &'static str)>| {
        let on = e.selection && !e.master_only;
        let mut v = vec![
            item("edit.cut", on),
            item("edit.copy", on),
            item("edit.paste", e.paste),
            item("edit.duplicate", on),
            item("edit.delete", on),
        ];
        if let Some((dup, del)) = label {
            v[3] = v[3].clone().labelled(dup);
            v[4] = v[4].clone().labelled(del);
        }
        v
    };
    match target {
        ContextTarget::Device => {
            let mut v = edit(None);
            v.extend([
                Spec::Separator,
                item("create.instrument", true),
                item("create.effect", true),
                item("rack.auto_route", e.selection),
                item("rack.assign_send", e.assign_send),
                item("rack.disconnect", e.selection),
                item("rack.go_to_track", e.has_track),
            ]);
            v
        }
        ContextTarget::Rack => vec![
            item("edit.paste", e.paste),
            Spec::Separator,
            item("create.instrument", true),
            item("create.effect", true),
            Spec::Separator,
            item(
                "rack.add_column",
                e.columns < atomik_model::MAX_RACK_COLUMNS,
            ),
            item("rack.remove_column", e.columns > 1),
            Spec::Separator,
            item("edit.select_all", true),
        ],
        ContextTarget::TrackHeader => edit(Some(("Duplicate Track", "Delete Track"))),
        ContextTarget::Clip => {
            let mut v = edit(None);
            v.extend([
                Spec::Separator,
                item("sequencer.open_clip", e.open_clip),
                item("sequencer.split_at_playhead", true),
                item("sequencer.join", e.join),
                Spec::Separator,
                item("sequencer.toggle_mute", e.selection).labelled(if e.clips_muted {
                    "Unmute"
                } else {
                    "Mute"
                }),
            ]);
            v
        }
        ContextTarget::Lane => vec![
            item("edit.paste", e.paste),
            Spec::Separator,
            item("edit.select_all", true),
        ],
        ContextTarget::Notes => {
            let mut v = edit(None);
            v.extend([
                Spec::Separator,
                item("editor.quantize", e.selection),
                item("editor.toggle_note_mute", e.selection).labelled(if e.notes_muted {
                    "Unmute Notes"
                } else {
                    "Mute Notes"
                }),
                item("edit.select_all", true),
            ]);
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(target: ContextTarget, e: &MenuEnable) -> Vec<(&'static str, bool)> {
        specs(target, e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item {
                    command, enabled, ..
                } => Some((command, enabled)),
                Spec::Separator => None,
            })
            .collect()
    }

    #[test]
    fn the_device_menu_lists_the_commands_of_fr_229_in_order() {
        let e = MenuEnable {
            selection: true,
            paste: true,
            has_track: true,
            ..MenuEnable::default()
        };
        let got: Vec<&str> = ids(ContextTarget::Device, &e)
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(
            got,
            [
                "edit.cut",
                "edit.copy",
                "edit.paste",
                "edit.duplicate",
                "edit.delete",
                "create.instrument",
                "create.effect",
                "rack.auto_route",
                "rack.assign_send",
                "rack.disconnect",
                "rack.go_to_track",
            ]
        );
    }

    #[test]
    fn enabling_follows_the_selection_the_clipboard_and_the_device() {
        let none = MenuEnable::default();
        let list = ids(ContextTarget::Device, &none);
        let on = |id: &str| list.iter().find(|(c, _)| *c == id).unwrap().1;
        assert!(!on("edit.cut") && !on("edit.copy") && !on("edit.paste"));
        assert!(!on("rack.disconnect") && !on("rack.go_to_track"));
        assert!(on("create.instrument") && on("create.effect"));
        assert!(!on("rack.auto_route"), "nothing selected, nothing to route");
        let some = MenuEnable {
            selection: true,
            paste: true,
            has_track: true,
            ..MenuEnable::default()
        };
        let list = ids(ContextTarget::Device, &some);
        let on = |id: &str| list.iter().find(|(c, _)| *c == id).unwrap().1;
        assert!(
            on("edit.cut")
                && on("edit.paste")
                && on("rack.disconnect")
                && on("rack.auto_route")
                && on("rack.go_to_track")
        );
    }

    #[test]
    fn the_rack_background_menu_offers_paste_create_columns_and_select_all() {
        let got: Vec<&str> = ids(ContextTarget::Rack, &MenuEnable::default())
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(
            got,
            [
                "edit.paste",
                "create.instrument",
                "create.effect",
                "rack.add_column",
                "rack.remove_column",
                "edit.select_all"
            ]
        );
    }

    #[test]
    fn remove_column_needs_two_columns_and_add_column_needs_room() {
        let on = |columns: u16, id: &str| {
            ids(
                ContextTarget::Rack,
                &MenuEnable {
                    columns,
                    ..Default::default()
                },
            )
            .into_iter()
            .find(|(c, _)| *c == id)
            .map(|(_, on)| on)
            .unwrap()
        };
        assert!(!on(1, "rack.remove_column") && on(1, "rack.add_column"));
        assert!(on(2, "rack.remove_column"));
        assert!(!on(atomik_model::MAX_RACK_COLUMNS, "rack.add_column"));
    }

    #[test]
    fn the_track_header_menu_says_track() {
        let e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let labels: Vec<Option<&str>> = specs(ContextTarget::TrackHeader, &e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item { label, .. } => Some(label),
                Spec::Separator => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                None,
                None,
                None,
                Some("Duplicate Track"),
                Some("Delete Track")
            ]
        );
    }

    #[test]
    fn the_clip_menu_enables_join_and_open_by_selection() {
        let mut e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let list = ids(ContextTarget::Clip, &e);
        let on = |l: &[(&str, bool)], id: &str| l.iter().find(|(c, _)| *c == id).unwrap().1;
        assert!(!on(&list, "sequencer.join") && !on(&list, "sequencer.open_clip"));
        assert!(on(&list, "sequencer.split_at_playhead"));
        e.join = true;
        e.open_clip = true;
        let list = ids(ContextTarget::Clip, &e);
        assert!(on(&list, "sequencer.join") && on(&list, "sequencer.open_clip"));
    }

    fn labels(target: ContextTarget, e: &MenuEnable) -> Vec<(&'static str, Option<&'static str>)> {
        specs(target, e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item { command, label, .. } => Some((command, label)),
                Spec::Separator => None,
            })
            .collect()
    }

    #[test]
    fn the_clip_menu_labels_the_mute_item_by_the_toggle_result() {
        let mut e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let find = |e: &MenuEnable| {
            labels(ContextTarget::Clip, e)
                .into_iter()
                .find(|(c, _)| *c == "sequencer.toggle_mute")
                .unwrap()
                .1
        };
        assert_eq!(find(&e), Some("Mute"));
        e.clips_muted = true;
        assert_eq!(find(&e), Some("Unmute"));
        assert!(
            ids(ContextTarget::Clip, &MenuEnable::default())
                .contains(&("sequencer.toggle_mute", false)),
            "disabled without a selection"
        );
    }

    #[test]
    fn the_notes_menu_labels_the_mute_item_by_the_toggle_result() {
        let mut e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let find = |e: &MenuEnable| {
            labels(ContextTarget::Notes, e)
                .into_iter()
                .find(|(c, _)| *c == "editor.toggle_note_mute")
                .unwrap()
                .1
        };
        assert_eq!(find(&e), Some("Mute Notes"));
        e.notes_muted = true;
        assert_eq!(find(&e), Some("Unmute Notes"));
    }
}
