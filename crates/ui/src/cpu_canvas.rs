//! A `vello_cpu` [`Canvas`]: rasterises a paint on the CPU, for PNG previews, snapshots and the
//! headless screenshots of `atomik-mcp`. Behind the `cpu-canvas` feature, so the app does not
//! link `vello_cpu`.

use crate::kurbo::{Affine, Stroke};
use crate::peniko::Brush;
use crate::{Canvas, Geom, TextEngine, TextRun};
use vello_cpu::{Pixmap, RenderContext, Resources};

/// A [`Canvas`] that rasterises into a `vello_cpu` pixmap; call [`CpuCanvas::finish`] for the pixels.
pub struct CpuCanvas<'a> {
    ctx: RenderContext,
    resources: Resources,
    text: &'a mut TextEngine,
    transforms: Vec<Affine>,
}

fn paint(b: &Brush) -> vello_cpu::PaintType {
    match b {
        Brush::Solid(c) => vello_cpu::PaintType::Solid(*c),
        Brush::Gradient(g) => vello_cpu::PaintType::Gradient(g.clone()),
        Brush::Image(_) => vello_cpu::PaintType::Solid(crate::peniko::Color::TRANSPARENT),
    }
}

impl<'a> CpuCanvas<'a> {
    /// A canvas of `width` x `height` physical pixels that shapes text with `text`.
    pub fn new(width: u16, height: u16, text: &'a mut TextEngine) -> Self {
        Self {
            ctx: RenderContext::new(width, height),
            resources: Resources::new(),
            text,
            transforms: vec![Affine::IDENTITY],
        }
    }

    fn current(&self) -> Affine {
        *self.transforms.last().expect("transform stack")
    }

    /// Renders and returns the pixmap.
    pub fn finish(mut self) -> Pixmap {
        self.ctx.flush();
        let mut pixmap = Pixmap::new(self.ctx.width(), self.ctx.height());
        self.ctx.render(&mut pixmap, &mut self.resources);
        pixmap
    }
}

impl Canvas for CpuCanvas<'_> {
    fn fill(&mut self, shape: &Geom, brush: &Brush) {
        self.ctx.set_transform(self.current());
        self.ctx.set_paint(paint(brush));
        self.ctx.fill_path(&shape.to_path());
    }
    fn stroke(&mut self, shape: &Geom, stroke: &Stroke, brush: &Brush) {
        self.ctx.set_transform(self.current());
        self.ctx.set_paint(paint(brush));
        self.ctx.set_stroke(stroke.clone());
        self.ctx.stroke_path(&shape.to_path());
    }
    fn push_clip(&mut self, shape: &Geom) {
        self.ctx.set_transform(self.current());
        self.ctx.push_clip_layer(&shape.to_path());
    }
    fn pop_clip(&mut self) {
        self.ctx.pop_layer();
    }
    fn push_transform(&mut self, transform: Affine) {
        let t = self.current() * transform;
        self.transforms.push(t);
    }
    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }
    fn text(&mut self, run: &TextRun) {
        self.ctx.set_transform(self.current());
        self.ctx.set_paint(vello_cpu::PaintType::Solid(run.color));
        for r in self.text.shape(run) {
            let glyphs = r.glyphs.iter().map(|g| vello_cpu::Glyph {
                id: g.id,
                x: g.x,
                y: g.y,
            });
            let _ = self
                .ctx
                .glyph_run(&mut self.resources, &r.font)
                .font_size(r.font_size)
                .normalized_coords(&r.coords)
                .fill_glyphs(glyphs);
        }
    }
}
