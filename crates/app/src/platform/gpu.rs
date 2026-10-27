//! vello on wgpu (through `vello::wgpu`): one shared context, a surface per window, and the
//! offscreen screenshot.
//!
//! Not unit-tested (needs a GPU); it compiles on every target and the user checks it.

use std::sync::Arc;
use vello::peniko::Color;
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};
use winit::window::Window;

/// What happened to a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameResult {
    /// Shown.
    Presented,
    /// Skipped (occluded, timeout); try again later.
    Skipped,
    /// The surface was reconfigured; redraw.
    Reconfigured,
    /// The surface is lost; recreate the GPU state.
    Lost,
}

fn renderer_options() -> RendererOptions {
    RendererOptions {
        antialiasing_support: AaSupport::area_only(),
        ..RendererOptions::default()
    }
}

fn params(base: Color, width: u32, height: u32) -> RenderParams {
    RenderParams {
        base_color: base,
        width,
        height,
        antialiasing_method: AaConfig::Area,
    }
}

/// The GPU state every window shares: one instance and adapter, one device, and one vello
/// renderer per device (the renderer's pipelines are built once, not once per window).
pub struct GpuContext {
    context: RenderContext,
    /// Indexed like `context.devices`; created the first time a surface uses the device.
    renderers: Vec<Option<Renderer>>,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext")
            .field("devices", &self.context.devices.len())
            .finish_non_exhaustive()
    }
}

/// One window's surface. Drawn with the [`GpuContext`] that created it.
pub struct WindowSurface {
    surface: RenderSurface<'static>,
}

impl std::fmt::Debug for WindowSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowSurface")
            .field(
                "size",
                &(self.surface.config.width, self.surface.config.height),
            )
            .finish_non_exhaustive()
    }
}

impl Default for GpuContext {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuContext {
    /// An empty context: the device is picked when the first surface is created.
    pub fn new() -> Self {
        Self {
            context: RenderContext::new(),
            renderers: Vec::new(),
        }
    }

    /// Creates the surface (vsync: `AutoVsync`) for `window`, and the renderer of its device
    /// when no window has needed it yet.
    pub fn create_surface(
        &mut self,
        window: Arc<Window>,
        width: u32,
        height: u32,
    ) -> Result<WindowSurface, String> {
        let surface = pollster::block_on(self.context.create_surface(
            window,
            width.max(1),
            height.max(1),
            wgpu::PresentMode::AutoVsync,
        ))
        .map_err(|e| format!("GPU surface: {e}"))?;
        self.renderer_for(surface.dev_id)?;
        Ok(WindowSurface { surface })
    }

    /// Switches `surface` between presenting in step with the monitor and presenting at once.
    pub fn set_vsync(&mut self, surface: &mut WindowSurface, vsync: bool) {
        let mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        if surface.surface.config.present_mode != mode {
            self.context.set_present_mode(&mut surface.surface, mode);
        }
    }

    /// The renderer of device `dev_id`, built on first use.
    fn renderer_for(&mut self, dev_id: usize) -> Result<&mut Renderer, String> {
        if self.renderers.len() <= dev_id {
            self.renderers.resize_with(dev_id + 1, || None);
        }
        let slot = &mut self.renderers[dev_id];
        if slot.is_none() {
            let device = &self.context.devices[dev_id].device;
            *slot = Some(
                Renderer::new(device, renderer_options()).map_err(|e| format!("renderer: {e}"))?,
            );
        }
        slot.as_mut().ok_or_else(|| "renderer".to_string())
    }

    /// Resizes `surface` (physical pixels).
    pub fn resize(&mut self, surface: &mut WindowSurface, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        let s = &mut surface.surface;
        if (w, h) != (s.config.width, s.config.height) {
            self.context.resize_surface(s, w, h);
        }
    }

    /// Renders `scene` at the surface size and presents it.
    pub fn render(
        &mut self,
        surface: &mut WindowSurface,
        scene: &Scene,
        base: Color,
    ) -> FrameResult {
        let surface = &mut surface.surface;
        let texture = match surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return FrameResult::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.context.configure_surface(surface);
                return FrameResult::Reconfigured;
            }
            wgpu::CurrentSurfaceTexture::Lost => return FrameResult::Lost,
            #[allow(unreachable_patterns)] // Future variants: skip the frame.
            _ => return FrameResult::Skipped,
        };
        if self.renderer_for(surface.dev_id).is_err() {
            return FrameResult::Skipped;
        }
        let Some(renderer) = self.renderers[surface.dev_id].as_mut() else {
            return FrameResult::Skipped;
        };
        let handle = &self.context.devices[surface.dev_id];
        let (device, queue) = (&handle.device, &handle.queue);
        let (w, h) = (surface.config.width, surface.config.height);
        if renderer
            .render_to_texture(
                device,
                queue,
                scene,
                &surface.target_view,
                &params(base, w, h),
            )
            .is_err()
        {
            return FrameResult::Skipped;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("atomik blit"),
        });
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        surface
            .blitter
            .copy(device, &mut encoder, &surface.target_view, &view);
        queue.submit([encoder.finish()]);
        queue.present(texture);
        FrameResult::Presented
    }
}

/// Renders `scene` offscreen at `width`×`height` and returns RGBA8 pixels (row-major).
pub fn render_offscreen(
    scene: &Scene,
    base: Color,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let mut context = RenderContext::new();
    let id = pollster::block_on(context.device(None)).ok_or("no compatible GPU device")?;
    let handle = &context.devices[id];
    let (device, queue) = (&handle.device, &handle.queue);
    let mut renderer =
        Renderer::new(device, renderer_options()).map_err(|e| format!("renderer: {e}"))?;
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("atomik screenshot"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer
        .render_to_texture(device, queue, scene, &view, &params(base, width, height))
        .map_err(|e| format!("render: {e}"))?;
    let row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("atomik screenshot readback"),
        size: u64::from(row) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("atomik screenshot copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("GPU poll: {e}"))?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("map: {e}"))?;
    let mapped = buffer
        .get_mapped_range(..)
        .map_err(|e| format!("mapped range: {e:?}"))?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        let start = y * row as usize;
        pixels.extend_from_slice(&mapped[start..start + width as usize * 4]);
    }
    Ok(pixels)
}

/// Writes RGBA8 pixels as a PNG.
pub fn write_png(
    path: &std::path::Path,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels).map_err(|e| e.to_string())
}
