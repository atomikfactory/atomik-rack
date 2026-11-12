//! The song key and the chord lane in the app core: setting the key, applying what the key and chord
//! picker built, **Detect Song Key**, and the browser's `key:song` filter.
//!
//! Every change is one undo step and says in the status line what it did. Detect Song Key reads
//! the notes of every note clip ([`atomik_model::key_detect`]) and, because a toast holds no
//! buttons, sets the key it finds as one undo step instead of proposing it; Undo puts the old key
//! back. A song with no notes but with audio clips reads the key from the audio instead (Detect Key
//! from Audio, on a worker: see [`super::chords`]); a song with neither is left alone.

use super::*;
use atomik_model::harmony::{Harmony, Key};
use atomik_model::{SongCommand, detect_song_key};

/// How sure a key guess is, in words for the status line.
fn confidence_word(confidence: f64) -> &'static str {
    if confidence >= 0.75 {
        "a clear fit"
    } else if confidence >= 0.5 {
        "a likely fit"
    } else {
        "a rough guess"
    }
}

impl<S: EngineSink> AppCore<S> {
    /// The song's key, key changes and chords.
    pub fn harmony(&self) -> &Harmony {
        &self.doc().song.harmony
    }

    /// The key at the playhead.
    pub fn key_at_playhead(&self) -> Option<Key> {
        self.harmony().key_at(self.position().max(0))
    }

    /// Tells the rack the song key and the chord at the playhead, for the SONG tags of the
    /// devices that follow the song (from [`Self::frame`]; the view repaints only on a change).
    pub(super) fn feed_song_harmony(&mut self) {
        let tick = self.position().max(0);
        let h = &self.doc().song.harmony;
        let key = h.key_at(tick).map(Key::short);
        let chord = h.chord_at(tick).map(|c| c.symbol());
        self.view.set_song_harmony(key, chord);
    }

    /// Makes `harmony` the song's key and chords (one undo step).
    ///
    /// # Errors
    /// The model refuses it (an overlap, a chromatic key); the reason is also the toast and the
    /// song is unchanged.
    pub fn set_harmony(&mut self, harmony: Harmony) -> Result<(), String> {
        if &harmony == self.harmony() {
            return Ok(());
        }
        match self.apply_command(Command::Song(SongCommand::SetHarmony(harmony))) {
            Ok(()) => Ok(()),
            Err(e) => {
                let why = format!("Cannot change the key and chords: {e}");
                self.report(why.clone());
                Err(why)
            }
        }
    }

    /// Sets (or clears) the song key (one undo step).
    ///
    /// # Errors
    /// See [`Self::set_harmony`].
    pub fn set_song_key(&mut self, key: Option<Key>) -> Result<(), String> {
        let harmony = self.harmony().with_key(key)?;
        self.set_harmony(harmony)?;
        self.report(match key {
            Some(k) => format!("Song key: {}", k.name()),
            None => "Song key cleared".to_owned(),
        });
        Ok(())
    }

    /// Estimates the key from every note clip and makes it the song key (one undo step). Returns
    /// the key it set, `None` when there are no notes to read or the key was already that one.
    /// With no notes but some audio, the key is read from the audio on a worker (and set when it
    /// is done).
    pub fn detect_song_key(&mut self) -> Option<Key> {
        let Some(guess) = detect_song_key(&self.doc().song) else {
            let has_audio = self.doc().song.tracks.iter().any(|t| {
                t.lanes.iter().any(|l| {
                    l.clips
                        .iter()
                        .any(|c| matches!(c.content, atomik_model::ClipContent::Audio(_)))
                })
            });
            if !(has_audio && self.detect_key_from_audio()) {
                self.report("No notes to read a key from");
            }
            return None;
        };
        if self.harmony().key == Some(guess.key) {
            self.report(format!("The song key is already {}", guess.key.name()));
            return None;
        }
        let harmony = self.harmony().with_key(Some(guess.key)).ok()?;
        self.set_harmony(harmony).ok()?;
        self.report(format!(
            "Song key set to {} ({}); Undo puts the old key back",
            guess.key.name(),
            confidence_word(guess.confidence)
        ));
        Some(guess.key)
    }

    /// Tells the file browser the tags of the song key, for a `key:song` search.
    pub(super) fn sync_song_key_filter(&mut self) {
        let tags = self
            .doc()
            .song
            .harmony
            .key_at(0)
            .map(|k| k.tags())
            .unwrap_or_default();
        self.file_browser.set_song_key(tags);
    }
}
