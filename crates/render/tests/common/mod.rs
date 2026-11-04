//! Shared by the render tests that bounce a project in-process: [`render_guarded`] is
//! `render_doc` with the allocation guard around every engine render call, so a bounce also
//! proves that the render path of that project allocates nothing. A test binary that uses it
//! installs the guard once with `atomik_testkit::install_alloc_guard!();`.
#![allow(dead_code, reason = "each test binary uses a subset")]

use atomik_device_sdk::Registry;
use atomik_model::RackDoc;
use atomik_render::plugin_pass::with_render_guard;
use atomik_render::project::{ProjectOptions, render_doc};
use atomik_render::{RenderError, Rendered};
use atomik_testkit::no_alloc::assert_no_alloc;

/// [`render_doc`] with the render path of every batch under `assert_no_alloc`.
pub fn render_guarded(
    doc: &RackDoc,
    registry: &Registry,
    options: &ProjectOptions,
) -> Result<Rendered, RenderError> {
    with_render_guard(
        |render| assert_no_alloc(render),
        || render_doc(doc, registry, options),
    )
}

/// [`atomik_render::project::render_doc_in`] (a project with a bundle) with the render path of
/// every batch under `assert_no_alloc`.
pub fn render_guarded_in(
    doc: &RackDoc,
    registry: &Registry,
    options: &ProjectOptions,
    bundle_root: Option<&std::path::Path>,
) -> Result<Rendered, RenderError> {
    with_render_guard(
        |render| assert_no_alloc(render),
        || atomik_render::project::render_doc_in(doc, registry, options, bundle_root),
    )
}
