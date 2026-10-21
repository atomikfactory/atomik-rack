//! Atomik Rack file library: the data behind the file browser.
//!
//! Pure and headless: no threads, no disk access, no clock. The app lists folders and probes
//! files on its workers and hands the results here as [`FileRow`]s; the UI filters them with
//! [`Query`] and orders them with [`sort_rows`]; tags, favourites and bookmarked places live in
//! a [`LibraryDb`] that the app saves as JSON.
//!
//! # Pieces
//!
//! - [`classify`] / [`FileKind`] / [`AudioFormat`]: which directory entries become rows.
//! - [`PathKey`]: the normalised path used as the database key (lexical, never touches disk).
//! - [`Tag`]: a validated, case-insensitive tag name.
//! - [`LibraryDb`]: favourites, tags and bookmarked places, saved as deterministic JSON.
//! - [`Query`]: the search field language (`words`, `#tag`, `fav`), allocation-free per row.
//! - [`natural_cmp`] / [`sort_rows`]: natural, total, deterministic ordering of rows.
//!
//! Collections are ordered (`BTreeMap`), so output never depends on hash order.
//!
//! The public signatures in this crate are a contract shared by `atomik-audio-file`,
//! `atomik-ui` and `atomik-app` (see the file browser plan); change them only deliberately.

mod db;
mod path_key;
mod query;
mod row;
mod sort;
mod tag;

pub use db::{LIBRARY_VERSION, LibraryDb, LibraryError, Mark};
pub use path_key::PathKey;
pub use query::Query;
pub use row::{AudioFormat, AudioInfo, FileKind, FileRow, classify};
pub use sort::{SortKey, natural_cmp, sort_rows};
pub use tag::{MAX_TAG_CHARS, Tag, TagError};
