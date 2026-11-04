//! The application layer above the songs.
//!
//! A [`Shell`] is one song: its document, its panes and panels, its undo history and the
//! windows it lays them out in. A [`Workspace`] holds any number of them and what they share:
//! which one has the audio, which one the keys go to, and the questions that span the songs
//! (quitting asks about every song with unsaved changes).
//!
//! # Audio
//!
//! The audio device is unique: only the song that has the **play focus** owns a running
//! stream, the MIDI inputs and the control surfaces. Every other song runs on a session that
//! is switched off (it plays nothing and holds no port), the same state a song is in when its
//! stream failed to open. Giving the focus to another song ([`Workspace::focus`]) closes the
//! stream of the old one before the new one opens it, with the settings the old one ran with,
//! and rebuilds the new song's engine, as a change of the audio devices does. The transport of
//! the song that lost the focus stops; its plugin instances stay loaded.
//!
//! # Routing
//!
//! The platform layer works with one song at a time: [`Workspace::select`] names the song the
//! next calls are for, and everything that is not a workspace method reaches that song's
//! [`Shell`] through `Deref`. The methods that return requests are the workspace's own: they
//! turn what a song asks for ("quit", "close this song") into what the whole workspace does.
//!
//! # What the songs share
//!
//! Each song reads the keymap, the workflow preferences and the layout of the window when it
//! opens, and [`Workspace::sync_preferences`] carries a change made in one song to the others.
//! Settings, the library and the recent list are files the songs read and write themselves.

use crate::commands::AppCommand;
use crate::session::AudioBackend;
use crate::shell::{PendingAction, Shell, ShellRequest, SongTarget};
use crate::windows::{DocId, WindowHit, WindowKind};
use atomik_ui::UiEvent;
use std::ops::{Deref, DerefMut};
use std::path::Path;

/// Makes a song: given the song the new one is opened from (for the settings it copies) and the
/// id it gets. The song starts as a new, empty one and runs silent (the workspace gives it the
/// audio when it is focused).
pub type SongFactory<B> = Box<dyn FnMut(&Shell<B>, DocId) -> Shell<B>>;

/// One open song.
struct Song<B: AudioBackend> {
    id: DocId,
    shell: Shell<B>,
}

/// The open songs and what they share. See the module docs.
pub struct Workspace<B: AudioBackend> {
    songs: Vec<Song<B>>,
    /// The song calls without an explicit song are for ([`Self::select`]).
    routed: DocId,
    /// The song that has the audio and the MIDI input.
    play: DocId,
    next: u32,
    make: SongFactory<B>,
    /// The song whose question about unsaved changes is on the screen, for the answer.
    asking: Option<DocId>,
    /// The songs that answered Don't Save to the question of a quit that is in progress.
    declined: Vec<DocId>,
}

impl<B: AudioBackend> std::fmt::Debug for Workspace<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field(
                "songs",
                &self.songs.iter().map(|s| s.id).collect::<Vec<_>>(),
            )
            .field("routed", &self.routed)
            .field("play", &self.play)
            .finish_non_exhaustive()
    }
}

impl<B: AudioBackend> Deref for Workspace<B> {
    type Target = Shell<B>;

    fn deref(&self) -> &Shell<B> {
        &self.songs[self.index_of(self.routed)].shell
    }
}

impl<B: AudioBackend> DerefMut for Workspace<B> {
    fn deref_mut(&mut self) -> &mut Shell<B> {
        let i = self.index_of(self.routed);
        &mut self.songs[i].shell
    }
}

impl<B: AudioBackend> Workspace<B> {
    /// A workspace of one song, [`DocId::FIRST`], which has the audio. `make` builds the songs
    /// opened later.
    pub fn new(first: Shell<B>, make: SongFactory<B>) -> Self {
        let mut workspace = Self {
            songs: vec![Song {
                id: DocId::FIRST,
                shell: first,
            }],
            routed: DocId::FIRST,
            play: DocId::FIRST,
            next: DocId::FIRST.0 + 1,
            make,
            asking: None,
            declined: Vec::new(),
        };
        workspace.share_song_list();
        workspace
    }

    fn index_of(&self, id: DocId) -> usize {
        self.songs.iter().position(|s| s.id == id).unwrap_or(0)
    }

    fn has(&self, id: DocId) -> bool {
        self.songs.iter().any(|s| s.id == id)
    }

    /// The open songs, in the order they were opened.
    pub fn ids(&self) -> Vec<DocId> {
        self.songs.iter().map(|s| s.id).collect()
    }

    /// How many songs are open.
    pub fn len(&self) -> usize {
        self.songs.len()
    }

    /// Whether no song is open (never: the last song cannot be closed).
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }

    /// The song calls are routed to now.
    pub fn routed(&self) -> DocId {
        self.routed
    }

    /// The song that has the audio and the MIDI input.
    pub fn play_focus(&self) -> DocId {
        self.play
    }

    /// The song `id`, when it is open.
    pub fn song(&self, id: DocId) -> Option<&Shell<B>> {
        self.songs.iter().find(|s| s.id == id).map(|s| &s.shell)
    }

    /// The song `id` for modification, when it is open.
    pub fn song_mut(&mut self, id: DocId) -> Option<&mut Shell<B>> {
        self.songs
            .iter_mut()
            .find(|s| s.id == id)
            .map(|s| &mut s.shell)
    }

    /// Routes the calls that follow to song `id` (no audio changes hands). False when no such
    /// song is open: the routing is then unchanged.
    pub fn select(&mut self, id: DocId) -> bool {
        if self.has(id) {
            self.carry_clipboard(self.routed, id);
            self.routed = id;
            true
        } else {
            false
        }
    }

    /// The clipboard is the application's, not one song's: what was last copied in the song the
    /// calls were routed to is what the song they are routed to next pastes (a copy of takes,
    /// clips or devices crosses to another open song). An empty clipboard leaves the other as
    /// it was.
    fn carry_clipboard(&mut self, from: DocId, to: DocId) {
        let (a, b) = (self.index_of(from), self.index_of(to));
        if a == b || !self.has(from) {
            return;
        }
        let Some(data) = self.songs[a].shell.core().clipboard().get().cloned() else {
            return;
        };
        let target = self.songs[b].shell.core_mut().clipboard_mut();
        if target.get() != Some(&data) {
            target.set(data);
        }
    }

    /// The name each song shows in the Window menu, in order.
    pub fn titles(&self) -> Vec<String> {
        self.songs.iter().map(|s| s.shell.song_name()).collect()
    }

    /// Hands every song the list of the open songs (for its Window menu).
    fn share_song_list(&mut self) {
        let titles = self.titles();
        for (i, song) in self.songs.iter_mut().enumerate() {
            song.shell.set_song_list(titles.clone(), i);
        }
    }

    /// The song `id` takes the audio from the one that has it (the keys follow too): the
    /// old song's stream, MIDI inputs and control surfaces close, then the new song opens them
    /// with the same settings. Returns false when no such song is open.
    pub fn focus(&mut self, id: DocId) -> bool {
        if !self.has(id) {
            return false;
        }
        self.carry_clipboard(self.routed, id);
        self.routed = id;
        if self.play == id {
            return true;
        }
        let from = self.index_of(self.play);
        let to = self.index_of(id);
        let settings = self.songs[from].shell.audio_settings();
        self.songs[from].shell.release_audio();
        self.songs[to].shell.claim_audio(settings);
        self.play = id;
        true
    }

    /// Opens a new song in a window of its own and gives it the focus.
    pub fn new_song(&mut self) -> DocId {
        let id = DocId(self.next);
        self.next += 1;
        let from = self.index_of(self.play);
        let mut shell = (self.make)(&self.songs[from].shell, id);
        shell.start_silent();
        shell.adopt_preferences(&self.songs[from].shell);
        shell.adopt_layout(&self.songs[from].shell);
        self.songs.push(Song { id, shell });
        self.focus(id);
        self.share_song_list();
        id
    }

    /// Opens the project or patch `path` in a song of its own. A file that cannot be opened
    /// leaves no new song: the message is shown in the song that was asked.
    pub fn open_song(&mut self, path: &Path) -> Option<DocId> {
        let asked = self.routed;
        let id = self.new_song();
        let opened = self.song_mut(id).map_or(Ok(()), |s| s.open_song_file(path));
        match opened {
            Ok(()) => {
                self.share_song_list();
                Some(id)
            }
            Err(e) => {
                self.discard_song(id);
                if let Some(s) = self.song_mut(asked) {
                    s.notify(e.to_string());
                }
                None
            }
        }
    }

    /// Closes song `id` without asking about unsaved changes (the caller did): its windows go
    /// (the platform layer follows [`Self::ids`]), its plugin instances end and, when it had the
    /// audio, the song next to it takes the audio over. False for the last song, which stays.
    pub fn close_song(&mut self, id: DocId) -> bool {
        if self.songs.len() < 2 || !self.has(id) {
            return false;
        }
        self.discard_song(id);
        true
    }

    fn discard_song(&mut self, id: DocId) {
        let at = self.index_of(id);
        let had_audio = self.play == id;
        let mut song = self.songs.remove(at);
        let settings = song.shell.audio_settings();
        let ui_state = song.shell.take_ui_state_path();
        song.shell.shutdown();
        song.shell.core_mut().sink_mut().shutdown();
        song.shell.core_mut().shutdown_plugins();
        let recovery = song.shell.core().recovery_folder();
        drop(song);
        // A song that was closed on purpose needs no recovery copy of its own.
        if id != DocId::FIRST
            && let Some(dir) = recovery.filter(|d| {
                d.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("song-"))
            })
        {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.declined.retain(|d| *d != id);
        if self.asking == Some(id) {
            self.asking = None;
        }
        if self.songs.is_empty() {
            return;
        }
        // The window state file belongs to the first song to have it, whichever song that is.
        if let Some(path) = ui_state {
            self.songs[0].shell.set_ui_state_path(path);
        }
        let next = self.songs[at.min(self.songs.len() - 1)].id;
        if had_audio {
            let i = self.index_of(next);
            self.songs[i].shell.claim_audio(settings);
            self.play = next;
        }
        if !self.has(self.routed) {
            self.routed = self.play;
        }
        self.share_song_list();
    }

    /// The song after the one with the focus (the first after the last), or the one before it.
    pub fn neighbour(&self, forward: bool) -> DocId {
        let n = self.songs.len();
        let at = self.index_of(self.play);
        let to = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        self.songs[to].id
    }

    /// Carries a change of the keyboard shortcuts or the workflow preferences made in the song
    /// the calls are routed to over to the others. Call after every event; it costs a
    /// comparison per song.
    pub fn sync_preferences(&mut self) {
        let source = self.index_of(self.routed);
        for i in 0..self.songs.len() {
            if i == source {
                continue;
            }
            let (from, to) = two_mut(&mut self.songs, source, i);
            if from.shell.shared_preferences() != to.shell.shared_preferences() {
                to.shell.adopt_preferences(&from.shell);
            }
        }
    }

    /// Per-frame work of every song (the background songs finish their saves, imports and
    /// loads; only the one with the audio plays).
    pub fn frame_all(&mut self, now_seconds: f64) {
        for song in &mut self.songs {
            song.shell.frame_at(now_seconds);
        }
    }

    /// Reads the audio status of every song; the one with the audio last, so that its state is
    /// what the crash report names.
    pub fn poll_all(&mut self) {
        let play = self.play;
        for song in self.songs.iter_mut().filter(|s| s.id != play) {
            song.shell.poll_status();
        }
        let i = self.index_of(play);
        self.songs[i].shell.poll_status();
    }

    /// Whether any song is busy with background work.
    pub fn any_busy(&self) -> bool {
        self.songs.iter().any(|s| s.shell.is_busy())
    }

    /// Before the app exits: every song writes its state, closes its stream and ends its
    /// plugins.
    pub fn shutdown_all(&mut self) {
        let play = self.play;
        for song in &mut self.songs {
            song.shell.shutdown();
            song.shell.poll_status();
            if song.id == play {
                log::info!("at exit: {}", song.shell.status_text());
            }
            // MIDI, stream, then the engine, all dropped here on the UI thread; then the plugins
            // (editors closed, the crash journal removed: a clean exit).
            song.shell.core_mut().sink_mut().shutdown();
            song.shell.core_mut().shutdown_plugins();
        }
    }

    // ------------------------------------------------------------ requests

    /// Turns what the song at `routed` asked for into what the workspace does. `fresh` is
    /// false for the answer to a question the workspace asked itself.
    fn route(&mut self, requests: Vec<ShellRequest>, fresh: bool) -> Vec<ShellRequest> {
        let mut out = Vec::with_capacity(requests.len());
        for request in requests {
            match request {
                ShellRequest::Quit => out.extend(self.quit_step()),
                ShellRequest::NewSong => {
                    let id = self.new_song();
                    out.push(ShellRequest::FocusSong(id));
                }
                ShellRequest::CloseSong => {
                    // The question about unsaved changes was answered. The last song cannot
                    // be closed on its own: closing it is quitting.
                    if !self.close_song(self.routed) {
                        out.extend(self.quit_step());
                    }
                }
                ShellRequest::ShowSong(target) => {
                    let id = match target {
                        SongTarget::Next => self.neighbour(true),
                        SongTarget::Previous => self.neighbour(false),
                        SongTarget::Place(place) => match self.songs.get(place) {
                            Some(song) => song.id,
                            None => continue,
                        },
                    };
                    self.focus(id);
                    out.push(ShellRequest::FocusSong(id));
                }
                ShellRequest::ConfirmSave { then } => {
                    self.asking = Some(self.routed);
                    if fresh && then == PendingAction::Command(AppCommand::Quit) {
                        self.declined.clear();
                    }
                    out.push(ShellRequest::ConfirmSave { then });
                }
                ShellRequest::PickSaveFileThen { then } => {
                    self.asking = Some(self.routed);
                    out.push(ShellRequest::PickSaveFileThen { then });
                }
                other => out.push(other),
            }
        }
        out
    }

    /// A song asked to quit: the song next in line with unsaved changes that nobody answered
    /// for gets the focus and the question; with none left the app quits.
    fn quit_step(&mut self) -> Vec<ShellRequest> {
        let ids = self.ids();
        for id in ids {
            if self.declined.contains(&id) {
                continue;
            }
            let dirty = self.song_mut(id).is_some_and(|s| {
                s.core_mut().settle_save();
                s.core().is_dirty()
            });
            if dirty {
                self.focus(id);
                self.asking = Some(id);
                return vec![ShellRequest::ConfirmSave {
                    then: PendingAction::Command(AppCommand::Quit),
                }];
            }
        }
        vec![ShellRequest::Quit]
    }

    /// Selects the song that raised the question the answer is for.
    fn select_asking(&mut self) {
        if let Some(id) = self.asking {
            self.select(id);
        }
    }

    /// Requests the routed song raised between events (a folder dialog, for instance): the
    /// platform loop takes them every tick, song by song.
    pub fn take_requests(&mut self) -> Vec<ShellRequest> {
        let requests = self.deref_mut().take_requests();
        self.route(requests, true)
    }

    /// See [`Shell::handle`]; for the song the calls are routed to.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<ShellRequest> {
        let requests = self.deref_mut().handle(event);
        self.route(requests, true)
    }

    /// See [`Shell::handle_in`].
    pub fn handle_in(&mut self, window: WindowKind, event: &UiEvent) -> Vec<ShellRequest> {
        let requests = self.deref_mut().handle_in(window, event);
        self.route(requests, true)
    }

    /// See [`Shell::handle_routed`].
    pub fn handle_routed(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
        under: Option<WindowHit>,
    ) -> Vec<ShellRequest> {
        let requests = self.deref_mut().handle_routed(window, event, under);
        self.route(requests, true)
    }

    /// See [`Shell::execute`].
    pub fn execute(&mut self, command: AppCommand) -> Vec<ShellRequest> {
        let requests = self.deref_mut().execute(command);
        self.route(requests, true)
    }

    /// See [`Shell::confirmed`].
    pub fn confirmed(&mut self, command: AppCommand) -> Vec<ShellRequest> {
        let requests = self.deref_mut().confirmed(command);
        self.route(requests, false)
    }

    /// Save, in the question about unsaved changes (see [`Shell::save_then`]).
    pub fn save_then(&mut self, then: PendingAction) -> Vec<ShellRequest> {
        self.select_asking();
        let requests = self.deref_mut().save_then(then);
        self.route(requests, false)
    }

    /// The file picked for a save that continues with `then` (see [`Shell::save_to_then`]).
    pub fn save_to_then(&mut self, path: &Path, then: PendingAction) -> Vec<ShellRequest> {
        self.select_asking();
        let requests = self.deref_mut().save_to_then(path, then);
        self.route(requests, false)
    }

    /// Don't Save, in the question about unsaved changes (see [`Shell::discard_then`]).
    pub fn discard_then(&mut self, then: PendingAction) -> Vec<ShellRequest> {
        self.select_asking();
        if then == PendingAction::Command(AppCommand::Quit) {
            self.declined.push(self.routed);
        }
        let requests = self.deref_mut().discard_then(then);
        self.route(requests, false)
    }
}

/// Two different elements of a slice, mutably.
fn two_mut<T>(items: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    assert_ne!(a, b, "two different songs");
    if a < b {
        let (head, tail) = items.split_at_mut(b);
        (&mut head[a], &mut tail[0])
    } else {
        let (head, tail) = items.split_at_mut(a);
        (&mut tail[0], &mut head[b])
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
