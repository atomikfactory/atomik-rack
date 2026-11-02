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
        Brush::Image(brush) => image_paint(brush).unwrap_or(vello_cpu::PaintType::Solid(
            crate::peniko::Color::TRANSPARENT,
        )),
    }
}

/// An RGBA8 image brush as a `vello_cpu` paint; `None` for another pixel format or a side over
/// 65535 pixels.
fn image_paint(brush: &crate::peniko::ImageBrush) -> Option<vello_cpu::PaintType> {
    use crate::peniko::{ImageAlphaType, ImageFormat};
    use vello_cpu::{Image, ImageSource, PixelMetadata};
    if brush.image.format != ImageFormat::Rgba8 {
        return None;
    }
    let width = u16::try_from(brush.image.width).ok()?;
    let height = u16::try_from(brush.image.height).ok()?;
    let alpha_type: ImageAlphaType = brush.image.alpha_type;
    let bytes = brush.image.data.as_ref();
    if bytes.len() != usize::from(width) * usize::from(height) * 4 {
        return None;
    }
    let pixmap = Pixmap::from_parts(
        bytes.to_vec(),
        width,
        height,
        PixelMetadata::new(alpha_type, true),
    );
    Some(vello_cpu::PaintType::Image(Image {
        image: ImageSource::Pixmap(std::sync::Arc::new(pixmap)),
        sampler: brush.sampler,
    }))
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
        let shaped = self.text.shape_shared(run);
        for r in shaped.runs.iter() {
            let glyphs = r.glyphs.iter().map(|g| vello_cpu::Glyph {
                id: g.id,
                x: shaped.x + g.x,
                y: shaped.y + g.y,
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
