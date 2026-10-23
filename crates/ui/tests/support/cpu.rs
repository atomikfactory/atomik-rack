//! The `vello_cpu` backend for pixel previews and snapshots: it lives in
//! `atomik_ui::cpu_canvas` (feature `cpu-canvas`, which this crate's own tests turn on).

#[allow(unused_imports)] // not every test binary that shares `support` rasterises
pub use atomik_ui::cpu_canvas::CpuCanvas;
