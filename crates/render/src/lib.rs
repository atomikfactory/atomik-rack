//! Atomik Rack offline renderer: patch JSON to WAV.
//!
//! - [`patch`]: the serde model of the patch file, its validation and JSON-path errors.
//! - [`build`]: resolves a [`patch::Patch`] against a device [`Registry`] into a compiled plan.
//! - [`flat`]: the expanded rack (containers compiled away) as a patch.
//! - [`bundle`]: project folder bundles (layout rules and asset loading for bounces).
//! - [`plugin_pass`]: the bounce loop for projects with plugins (pre-roll, restarts, latency).
//! - [`project`]: `.atomik` projects: the song as engine input and the song bounce.
//! - [`samples`]: sampler sample maps from document zones (shared with the app).
//! - [`wav`]: interleaved WAV writing (32-bit float or 24-bit PCM) and reading.
//! - [`cli`]: the `atomik-render` command line (the binary is a thin wrapper around it).
//!
//! ```no_run
//! # fn main() -> Result<(), atomik_render::RenderError> {
//! let text = std::fs::read_to_string("examples/patches/minimal.json").unwrap();
//! let block = std::num::NonZeroUsize::new(512).unwrap();
//! let rendered = atomik_render::render_json(&text, &atomik_render::default_registry(), block)?;
//! assert_eq!(rendered.channels.len(), 2);
//! # Ok(()) }
//! ```

pub mod build;
pub mod bundle;
pub mod cli;
pub mod flat;
pub mod patch;
pub mod plugin_pass;
pub mod plugins;
pub mod project;
pub mod samples;
pub mod wav;

use atomik_device_sdk::Registry;
use build::{BuildError, Built};
use patch::{Patch, PatchError, Warning};
use std::num::NonZeroUsize;

/// A registry holding every built-in device (`atomik-devices`).
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    // The built-in set is fixed and its descriptors are tested in `atomik-devices`.
    atomik_devices::register_all(&mut registry).expect("built-in devices register cleanly");
    registry
}

/// The rendered audio of a patch.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// One buffer per output channel, all the same length.
    pub channels: Vec<Vec<f32>>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Warnings collected while building.
    pub warnings: Vec<Warning>,
    /// Plugin devices in the bounced rack (0 for a patch and for most projects). Their output is
    /// bit-identical only for deterministic plugins.
    pub plugins: usize,
}

/// Renders a built patch offline with host blocks of `host_block` frames. The output does not
/// depend on `host_block`.
pub fn render_built(built: Built, host_block: NonZeroUsize) -> Vec<Vec<f32>> {
    atomik_engine::render_offline(
        built.plan,
        built.config,
        built.frames,
        host_block.get(),
        &built.notes,
        &[],
    )
}

/// Parses, validates, builds and renders patch JSON.
pub fn render_json(
    text: &str,
    registry: &Registry,
    host_block: NonZeroUsize,
) -> Result<Rendered, RenderError> {
    let patch = Patch::from_json(text)?;
    let mut built = build::build(&patch, registry)?;
    let warnings = std::mem::take(&mut built.warnings);
    Ok(Rendered {
        channels: render_built(built, host_block),
        sample_rate: patch.sample_rate,
        warnings,
        plugins: 0,
    })
}

/// Any failure of the renderer. `Display` is `<location>: <message>`, where the location is a
/// JSON path for patch problems and a file path for I/O problems.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The patch file could not be read.
    #[error("{}: cannot read the patch: {source}", path.display())]
    Read {
        /// The patch file.
        path: std::path::PathBuf,
        /// The I/O error.
        source: std::io::Error,
    },
    /// The patch is malformed or invalid.
    #[error(transparent)]
    Patch(#[from] PatchError),
    /// The patch cannot be built into a valid graph.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// The project file is invalid, or its song cannot be compiled or queued. The message
    /// starts with the JSON path for file problems.
    #[error("{0}")]
    Project(String),
    /// The WAV file could not be written.
    #[error("{}: cannot write the WAV file: {source}", path.display())]
    Write {
        /// The output file.
        path: std::path::PathBuf,
        /// The WAV error.
        source: wav::WavError,
    },
}
