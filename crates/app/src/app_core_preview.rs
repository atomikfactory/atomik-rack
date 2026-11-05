//! The player preview: the play button on the panel of a note player (and `rack.preview_player`)
//! runs the player, and the players after it down to the instrument of its track, on a clock of
//! their own at the song tempo while the song transport stands still, so the notes can be heard
//! through the track instrument and channel without playing the track.
//!
//! The clock itself is the engine's (`EngineSink::set_previews`, one flag per device on the
//! control path); this module decides which devices run it and when it ends:
//!
//! * Pressing the button of a player starts its preview; pressing it again stops it. Only one
//!   preview runs: starting one first stops the other, so the new one begins at its pattern start.
//! * The song transport starting (the Play button, a recording, a followed MIDI clock) stops it,
//!   as does opening another song or restarting the audio engine, which end with a new engine.
//! * Deleting the previewed player stops it and silences the notes it had sounding (the player
//!   is gone and cannot close them). Changes to the stack under it (a player added, moved or
//!   deleted) are followed: the devices after it keep running.
//! * Stopping a preview needs no note off from here: the players close what they hold when their
//!   clock stops, and those notes go through the stack as any other.
//!
//! Nothing is saved and nothing in the document changes. A note-processing player (arpeggiator,
//! note repeater, note delay, Key & Chord, note filter) needs notes to process: with the player
//! selected, the notes played live on the Piano Keys or a MIDI keyboard go to it as they do with
//! the transport stopped, so they are heard through it while it previews.

use super::*;
use atomik_engine::DeviceInstanceId;
use atomik_model::{is_note_player, preview_chain};

/// The preview that runs.
#[derive(Debug, Clone)]
pub(super) struct PlayerPreview {
    /// The player whose button started it.
    device: DeviceKey,
    /// The devices that run the clock: the player and the players after it.
    chain: Vec<DeviceKey>,
    /// The document revision the chain was made at.
    revision: u64,
}

impl<S: EngineSink> AppCore<S> {
    /// The player whose preview runs, if any.
    pub fn previewing_player(&self) -> Option<&DeviceKey> {
        self.player_preview.as_ref().map(|p| &p.device)
    }

    /// Whether `key` is a note player of the rack (inside a container too).
    pub fn is_player_device(&self, key: &DeviceKey) -> bool {
        self.doc()
            .resolve(key)
            .and_then(|d| self.registry.descriptor(&d.type_id))
            .is_some_and(is_note_player)
    }

    /// The devices a preview of `key` runs: see [`atomik_model::preview_chain`]. A player inside
    /// a container runs alone.
    fn preview_chain_of(&self, key: &DeviceKey) -> Vec<DeviceKey> {
        let chain = preview_chain(self.doc(), &self.registry, key);
        if chain.is_empty() && self.is_player_device(key) {
            vec![key.clone()]
        } else {
            chain
        }
    }

    /// The engine ids of `chain` (a device the engine does not have yet is left out).
    fn preview_ids(&self, chain: &[DeviceKey]) -> Vec<DeviceInstanceId> {
        chain
            .iter()
            .filter_map(|k| self.ids.get(k).copied())
            .collect()
    }

    /// `rack.preview_player`: previews the selected device, or stops the preview that runs.
    pub fn preview_selected_player(&mut self) {
        match self.view.selected().cloned() {
            Some(key) => self.toggle_player_preview(&key),
            None => self.reject("Select a note player to preview".into()),
        }
    }

    /// The play button of player `key`: stops its preview when it runs, else starts it (after
    /// stopping any other).
    pub fn toggle_player_preview(&mut self, key: &DeviceKey) {
        if self.previewing_player() == Some(key) {
            self.stop_player_preview();
            return;
        }
        if !self.is_player_device(key) {
            self.reject("Only a note player can be previewed".into());
            return;
        }
        if self.playing {
            self.reject("Stop the song to preview a player: it plays with the transport".into());
            return;
        }
        if !self.sink.has_engine() {
            self.reject("Previewing needs audio: it is off or its stream did not open".into());
            return;
        }
        let chain = self.preview_chain_of(key);
        if chain.is_empty() {
            self.reject("This player is not in the sound path of the rack".into());
            return;
        }
        // The other preview ends first, so that a device both run starts from its pattern start.
        self.sink.set_previews(&[]);
        let ids = self.preview_ids(&chain);
        self.sink.set_previews(&ids);
        self.player_preview = Some(PlayerPreview {
            device: key.clone(),
            chain,
            revision: self.document.revision(),
        });
        self.view.set_previewing(Some(key.clone()));
    }

    /// Ends the preview that runs (nothing happens without one). The players close their notes
    /// when their clock stops.
    pub fn stop_player_preview(&mut self) {
        if self.player_preview.take().is_some() {
            self.sink.set_previews(&[]);
            self.view.set_previewing(None);
        }
    }

    /// Forgets the preview without telling the engine: a new engine has no clocks.
    pub(super) fn forget_player_preview(&mut self) {
        self.player_preview = None;
        self.view.set_previewing(None);
    }

    /// Once per frame: the song transport playing ends the preview, a deleted player ends it
    /// with its notes, and a changed stack under it is followed.
    pub(super) fn feed_player_preview(&mut self) {
        let Some(preview) = &self.player_preview else {
            return;
        };
        if self.playing {
            self.stop_player_preview();
            return;
        }
        if preview.revision == self.document.revision() {
            return;
        }
        let (device, old) = (preview.device.clone(), preview.chain.clone());
        let chain = self.preview_chain_of(&device);
        if chain.is_empty() {
            // The player is gone: it cannot close its notes, so everything is released.
            self.stop_player_preview();
            self.all_notes_off();
            return;
        }
        let revision = self.document.revision();
        if chain != old {
            let ids = self.preview_ids(&chain);
            self.sink.set_previews(&ids);
        }
        if let Some(p) = &mut self.player_preview {
            p.chain = chain;
            p.revision = revision;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::{RecordingSink, SinkCall};
    use atomik_core::NoteKind;
    use atomik_model::{Cable, SocketRef};
    use atomik_ui::TransportAction;

    const SEQ: &str = "atomik.step-sequencer";
    const ARP: &str = "atomik.arpeggiator";
    const SYNTH: &str = "atomik.poly-synth";

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    fn add(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        })
        .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
    }

    fn cable(c: &mut AppCore<RecordingSink>, from: &str, to: &str) {
        let end = |d: &str, s: &str| SocketRef {
            device: key(d),
            socket: s.to_string(),
        };
        c.apply_command(Command::Connect(Cable {
            from: end(from, "notes_out"),
            to: end(to, "notes_in"),
        }))
        .unwrap();
    }

    /// `seq -> arp -> synth`.
    fn stack() -> AppCore<RecordingSink> {
        let mut c = AppCore::new(RecordingSink::new());
        add(&mut c, SYNTH, "synth");
        add(&mut c, ARP, "arp");
        add(&mut c, SEQ, "seq");
        cable(&mut c, "arp", "synth");
        cable(&mut c, "seq", "arp");
        c.sink_mut().take();
        c
    }

    fn id(c: &AppCore<RecordingSink>, k: &str) -> DeviceInstanceId {
        c.ids[&key(k)]
    }

    fn previews(c: &mut AppCore<RecordingSink>) -> Vec<Vec<DeviceInstanceId>> {
        c.sink_mut()
            .take()
            .into_iter()
            .filter_map(|call| match call {
                SinkCall::PlayerPreviews(ids) => Some(ids),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_player_previews_with_the_players_after_it_and_a_second_press_stops_it() {
        let mut c = stack();
        let (seq, arp) = (id(&c, "seq"), id(&c, "arp"));
        let revision = c.document.revision();
        c.toggle_player_preview(&key("seq"));
        // The old preview ends first, then the chain starts: the player and the arp, not the synth.
        assert_eq!(previews(&mut c), vec![vec![], vec![seq, arp]]);
        assert_eq!(c.previewing_player(), Some(&key("seq")));
        assert_eq!(c.view().previewing(), Some(&key("seq")));
        c.toggle_player_preview(&key("seq"));
        assert_eq!(previews(&mut c), vec![vec![]]);
        assert_eq!(c.previewing_player(), None);
        assert_eq!(c.view().previewing(), None);
        // The document is untouched and the transport never moved.
        assert_eq!(c.document.revision(), revision);
        assert!(
            !c.sink_mut()
                .take()
                .iter()
                .any(|x| matches!(x, SinkCall::Transport(_)))
        );
    }

    #[test]
    fn starting_another_preview_stops_the_first_and_restarts_shared_players() {
        let mut c = stack();
        let (seq, arp) = (id(&c, "seq"), id(&c, "arp"));
        c.toggle_player_preview(&key("seq"));
        previews(&mut c);
        c.toggle_player_preview(&key("arp"));
        // Everything stops, then the arp alone starts again from the top.
        assert_eq!(previews(&mut c), vec![vec![], vec![arp]]);
        assert_eq!(c.previewing_player(), Some(&key("arp")));
        assert_ne!(seq, arp);
    }

    #[test]
    fn something_that_is_not_a_player_is_refused_and_nothing_reaches_the_engine() {
        let mut c = stack();
        c.toggle_player_preview(&key("synth"));
        c.toggle_player_preview(&key("nothing"));
        assert!(previews(&mut c).is_empty());
        assert_eq!(c.previewing_player(), None);
    }

    #[test]
    fn a_running_song_refuses_a_preview_and_the_transport_starting_ends_one() {
        let mut c = stack();
        c.transport_action(TransportAction::Play);
        c.sink_mut().take();
        c.toggle_player_preview(&key("seq"));
        assert!(previews(&mut c).is_empty());
        assert_eq!(c.previewing_player(), None);
        c.transport_action(TransportAction::Stop);
        c.toggle_player_preview(&key("seq"));
        c.sink_mut().take();
        c.transport_action(TransportAction::Play);
        let calls = c.sink_mut().take();
        let stop = calls
            .iter()
            .position(|x| matches!(x, SinkCall::PlayerPreviews(ids) if ids.is_empty()))
            .expect("the preview stops");
        let play = calls
            .iter()
            .position(|x| matches!(x, SinkCall::Transport(_)))
            .expect("the transport starts");
        assert!(stop < play, "the preview ends before the song starts");
        assert_eq!(c.previewing_player(), None);
    }

    #[test]
    fn the_transport_playing_by_itself_ends_the_preview_on_the_next_frame() {
        // A followed MIDI clock starts the transport without the Play button.
        let mut c = stack();
        c.toggle_player_preview(&key("seq"));
        c.sink_mut().take();
        c.playing = true;
        c.frame();
        assert_eq!(previews(&mut c), vec![vec![]]);
        assert_eq!(c.previewing_player(), None);
    }

    #[test]
    fn deleting_the_previewed_player_stops_it_and_releases_the_notes() {
        let mut c = stack();
        c.toggle_player_preview(&key("seq"));
        c.sink_mut().take();
        c.apply_command(Command::RemoveDevice { key: key("seq") })
            .unwrap();
        c.frame();
        let calls = c.sink_mut().take();
        assert!(
            calls
                .iter()
                .any(|x| matches!(x, SinkCall::PlayerPreviews(ids) if ids.is_empty()))
        );
        assert!(
            calls.iter().any(|x| matches!(
                x,
                SinkCall::LiveNote {
                    kind: NoteKind::AllOff,
                    ..
                }
            )),
            "no note hangs on the instrument"
        );
        assert_eq!(c.previewing_player(), None);
    }

    #[test]
    fn deleting_a_player_below_the_preview_keeps_the_rest_running() {
        let mut c = stack();
        let seq = id(&c, "seq");
        c.toggle_player_preview(&key("seq"));
        c.sink_mut().take();
        c.apply_command(Command::RemoveDevice { key: key("arp") })
            .unwrap();
        c.frame();
        let last = previews(&mut c).pop().expect("the set is updated");
        assert_eq!(last, vec![seq]);
        assert_eq!(c.previewing_player(), Some(&key("seq")));
    }

    #[test]
    fn a_new_engine_forgets_the_preview_without_telling_the_old_one() {
        let mut c = stack();
        c.toggle_player_preview(&key("seq"));
        c.sink_mut().take();
        c.rebuild_engine();
        assert_eq!(c.previewing_player(), None);
        assert!(previews(&mut c).is_empty());
    }

    #[test]
    fn without_audio_the_preview_is_refused() {
        let mut c = stack();
        c.sink_mut().no_engine = true;
        c.toggle_player_preview(&key("seq"));
        assert_eq!(c.previewing_player(), None);
        assert!(previews(&mut c).is_empty());
    }

    #[test]
    fn the_command_previews_the_selected_player() {
        let mut c = stack();
        c.preview_selected_player();
        assert_eq!(c.previewing_player(), None, "nothing is selected");
        c.view_mut().set_selected(Some(key("arp")));
        c.preview_selected_player();
        assert_eq!(c.previewing_player(), Some(&key("arp")));
        c.preview_selected_player();
        assert_eq!(c.previewing_player(), None);
    }
}
