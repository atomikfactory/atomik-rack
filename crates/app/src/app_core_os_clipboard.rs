//! Copy and paste between two running instances: the app core's side of [`crate::clipboard_os`].
//!
//! A Copy or Cut ([`AppCore::set_clipboard`]) fills the in-process clipboard and also writes an
//! envelope to the system clipboard. A Paste ([`AppCore::adopt_os_clipboard`]) first looks at the
//! system clipboard: an envelope there that differs from the text this instance already stands
//! for came from another instance, so it is decoded and replaces the in-process contents (a
//! repeated paste then takes the fast path); anything else, text of other programs included, is
//! ignored and the in-process clipboard is used as it is.
//!
//! Audio travels by reference: the envelope names the bundle folder the copied audio lives in,
//! and the paste fetches the files by content hash through the same path a paste from another
//! open song uses ([`AppCore::set_audio_lenders`]). A file that cannot be reached leaves the
//! pasted clip or zone as missing media, which the usual relink flow repairs, and the status line
//! says so. Every call into the system clipboard is allowed to fail and then the in-process
//! clipboard alone works.

use super::*;
use crate::clipboard::{ClipboardData, ClipboardKind};
use crate::clipboard_os::{self, Decoded, OsClipboard, Skipped, Source};
use atomik_audio_file::Bundle;
use atomik_model::AssetMeta;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long a look at the system clipboard for the Paste menu stays good.
const PEEK_TTL: Duration = Duration::from_millis(500);

impl<S: EngineSink> AppCore<S> {
    /// Replaces the system clipboard this core copies through and pastes from (tests give it an
    /// in-memory one).
    pub fn set_os_clipboard(&mut self, clipboard: Box<dyn OsClipboard>) {
        self.os_clipboard = std::cell::RefCell::new(clipboard);
        self.os_peek.set(None);
    }

    /// Makes `data` the clipboard: in this process, and for the kinds other instances can paste
    /// also as text on the system clipboard. A selection too big for the system clipboard stays
    /// in this instance and the status line says so.
    pub(super) fn set_clipboard(&mut self, data: ClipboardData) {
        let source = Source {
            rate: self.doc().sample_rate,
            tempo: self.doc().tempo_bpm,
            bundle: self.bundle_root(),
        };
        let encoded = clipboard_os::encode(&data, &source);
        self.clipboard.set(data);
        self.os_peek.set(None);
        let written = match &encoded {
            Ok(text) => self.os_clipboard.borrow_mut().write(text),
            Err(_) => false,
        };
        match encoded {
            Ok(text) if written => {
                self.clipboard.set_known(Some(text));
                return;
            }
            Err(Skipped::TooBig(bytes)) => self.report(format!(
                "Copied here only: {} MB is too much for the system clipboard",
                bytes.div_ceil(1024 * 1024)
            )),
            Err(Skipped::Failed(e)) => log::warn!("the copy is not shared: {e}"),
            Ok(_) | Err(Skipped::Local) => {}
        }
        // This copy stays here: whatever is on the system clipboard now is older than it, and a
        // Paste must not mistake that for a newer copy of another instance.
        let current = self.os_clipboard.borrow_mut().read();
        self.clipboard.set_known(current);
    }

    /// Before a Paste: if the system clipboard holds an envelope of another instance, makes it the
    /// in-process clipboard. `Err` is the message when that envelope cannot be used (the paste
    /// stops: the system clipboard is the newest copy).
    pub(super) fn adopt_os_clipboard(&mut self) -> Result<(), String> {
        let Some(text) = self.os_clipboard.borrow_mut().read() else {
            return Ok(());
        };
        if !text.starts_with(clipboard_os::MAGIC) || self.clipboard.known() == Some(text.as_str()) {
            return Ok(());
        }
        match clipboard_os::decode(&text, &self.registry) {
            Decoded::NotOurs => Ok(()),
            Decoded::Refused(message) => Err(message),
            Decoded::Data(data, audio) => {
                let data = *data;
                if let ClipboardData::Devices(s) | ClipboardData::Tracks(s) = &data {
                    let mut holder = atomik_model::empty_doc();
                    holder.devices.clone_from(&s.devices);
                    // The plugins this machine lacks get stand-ins built from their layouts.
                    atomik_model::register_stand_ins(&holder, &mut self.registry)
                        .map_err(|e| format!("The copied devices cannot be pasted: {e}"))?;
                }
                let missing = self.lend_from(audio.as_deref(), assets_of(&data));
                self.clipboard.set(data);
                self.clipboard.set_known(Some(text));
                self.os_peek.set(None);
                if missing > 0 {
                    self.report(format!(
                        "{missing} audio file{} of the copy could not be found: pasted as missing media, relink {}",
                        if missing == 1 { "" } else { "s" },
                        if missing == 1 { "it" } else { "them" },
                    ));
                }
                Ok(())
            }
        }
    }

    /// Adds the folder another instance copied its audio from to the folders audio is fetched
    /// from, and counts the assets in `assets` that this song lacks and that folder cannot give.
    fn lend_from(
        &mut self,
        folder: Option<&Path>,
        assets: Option<&BTreeMap<String, AssetMeta>>,
    ) -> usize {
        let Some(assets) = assets else {
            return 0;
        };
        let bundle = folder.and_then(|f| Bundle::open(f).ok());
        if let Some(b) = &bundle
            && !self.audio.lenders.iter().any(|l| l == b.root())
        {
            self.audio.lenders.push(b.root().to_path_buf());
        }
        assets
            .keys()
            .filter(|h| !self.doc().assets.contains_key(*h))
            .filter(|h| !bundle.as_ref().is_some_and(|b| b.exists(h)))
            .count()
    }

    /// The kind of another instance's copy waiting on the system clipboard, for the Paste menu.
    /// The system clipboard is looked at once per [`PEEK_TTL`], not on every menu build.
    pub(super) fn foreign_kind(&self) -> Option<ClipboardKind> {
        let now: Instant = crate::session::clock_now();
        if let Some((at, kind)) = self.os_peek.get()
            && now.saturating_duration_since(at) < PEEK_TTL
        {
            return kind;
        }
        let text = self.os_clipboard.borrow_mut().read();
        let kind = text
            .as_deref()
            .filter(|t| self.clipboard.known() != Some(*t))
            .and_then(clipboard_os::peek_kind);
        self.os_peek.set(Some((now, kind)));
        kind
    }
}

/// The pool entries a copy needs.
fn assets_of(data: &ClipboardData) -> Option<&BTreeMap<String, AssetMeta>> {
    match data {
        ClipboardData::Clips(s) => Some(&s.assets),
        ClipboardData::Devices(s) | ClipboardData::Tracks(s) => Some(&s.assets),
        _ => None,
    }
}
