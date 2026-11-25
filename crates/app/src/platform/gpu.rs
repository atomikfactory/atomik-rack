//! vello on wgpu (through `vello::wgpu`): one shared context, a surface per window, and the
//! offscreen screenshot.
//!
//! A device the driver took away (a reset, a driver update, a wake from sleep) is reported by
//! two callbacks that only set the flag of [`GpuHealth`]; the app notices it at the start of a
//! frame and builds the whole context again (see [`super::gpu_recovery`]). No surface or render
//! target is ever configured larger than the device's texture limit.
//!
//! At Low power (see [`crate::performance`]) a window's render target is smaller than its
//! surface: the scene is scaled down into it and the blit stretches it back over the surface
//! with linear filtering. The surface, the scene and everything the UI reports (pointer
//! positions, text positions, screenshots) stay in the window's own pixels; only the pixels
//! vello fills are fewer.
//!
//! The adapter is chosen here by the rule of [`super::gpu_select`] (never a software adapter
//! unless it is named in `WGPU_ADAPTER_NAME`), for the first start and for every rebuild alike;
//! when none is acceptable the surface cannot be made and the caller says why.
//!
//! The GPU parts are not unit-tested (they need a GPU); the flag and the decisions are, and the
//! user checks the rest (`ATOMIK_GPU_LOSS_EVERY` forces a loss).

use super::gpu_recovery::clamp_size;
use super::gpu_select::{self, AdapterDesc, Kind, Overrides};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::util::RenderSurface;
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
    /// The device or the surface is lost; the whole GPU state is built again.
    Lost,
}

/// What the graphics driver's callbacks recorded about one device: they set the flag and keep
/// the words, nothing else (no logging, no work), and the UI thread reads it every frame.
#[derive(Debug, Default)]
pub struct GpuHealth {
    lost: AtomicBool,
    /// Why the device was lost, until the UI thread has said it.
    reason: Mutex<Option<String>>,
    /// The errors that do not lose the device (a rejected command), each distinct text once,
    /// until the UI thread has said them.
    notes: Mutex<Notes>,
}

#[derive(Debug, Default)]
struct Notes {
    pending: Vec<String>,
    seen: Vec<String>,
}

/// The most distinct non-fatal error texts kept per device.
const MAX_NOTES: usize = 16;

impl GpuHealth {
    /// Records that the device is lost, with the reason (the first reason is kept).
    pub fn mark_lost(&self, why: &str) {
        if !self.lost.swap(true, Ordering::AcqRel)
            && let Ok(mut r) = self.reason.lock()
        {
            *r = Some(why.to_string());
        }
    }

    /// Whether the device was reported lost.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    /// The reason of the loss, once.
    #[must_use]
    pub fn take_reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|mut r| r.take())
    }

    /// Records an error that leaves the device usable.
    pub fn note(&self, text: &str) {
        if let Ok(mut n) = self.notes.lock()
            && n.seen.len() < MAX_NOTES
            && !n.seen.iter().any(|s| s == text)
        {
            n.seen.push(text.to_string());
            n.pending.push(text.to_string());
        }
    }

    /// The non-fatal errors recorded since the last call.
    #[must_use]
    pub fn take_notes(&self) -> Vec<String> {
        self.notes
            .lock()
            .map(|mut n| std::mem::take(&mut n.pending))
            .unwrap_or_default()
    }

    /// What the uncaptured-error callback does with `error`: running out of memory or an
    /// internal failure loses the device; a rejected command is only noted.
    fn on_error(&self, error: &wgpu::Error) {
        match error {
            wgpu::Error::Validation { description, .. } => self.note(description),
            other => self.mark_lost(&format!("graphics error: {other}")),
        }
    }

    /// Installs the two callbacks on `device`.
    fn watch(self: &Arc<Self>, device: &wgpu::Device) {
        let health = Arc::clone(self);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| health.on_error(&e)));
        let health = Arc::clone(self);
        device.set_device_lost_callback(move |reason, message| {
            health.mark_lost(&format!("{reason:?}: {message}"));
        });
    }
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

/// The graphics API an adapter is used through, as the log names it.
fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Dx12 => "DX12",
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Gl => "OpenGL",
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        _ => "other",
    }
}

/// The kind of adapter, as the log names it.
fn device_type_name(kind: wgpu::DeviceType) -> &'static str {
    match kind {
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "CPU",
        wgpu::DeviceType::Other => "other",
    }
}

/// The kind of an adapter for the selection rule.
fn kind_of(kind: wgpu::DeviceType) -> Kind {
    match kind {
        wgpu::DeviceType::DiscreteGpu => Kind::Discrete,
        wgpu::DeviceType::IntegratedGpu => Kind::Integrated,
        wgpu::DeviceType::VirtualGpu => Kind::Virtual,
        wgpu::DeviceType::Cpu => Kind::Cpu,
        wgpu::DeviceType::Other => Kind::Other,
    }
}

/// One adapter with the device and queue made on it.
struct DeviceHandle {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// The instance and the devices made on it. The adapter is chosen by [`gpu_select::choose`]
/// instead of the renderer library's own pick, so a software adapter is never used by chance.
struct Devices {
    instance: wgpu::Instance,
    devices: Vec<DeviceHandle>,
    /// The adapters seen by the last selection, for the message of a start that found none.
    found: Vec<AdapterDesc>,
    /// The adapters being listed on a worker since the start, taken by the first selection.
    listing: Option<std::thread::JoinHandle<Vec<wgpu::Adapter>>>,
}

impl Devices {
    fn new() -> Self {
        let backends = wgpu::Backends::from_env().unwrap_or_default();
        let flags = wgpu::InstanceFlags::from_build_config().with_env();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: None,
            backends,
            flags,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        Self {
            instance,
            devices: Vec::new(),
            found: Vec::new(),
            listing: None,
        }
    }

    /// Lists the adapters of the machine on a worker, so that the list is there (or nearly) when
    /// the first window asks for a device: the driver's first look at the machine is the slowest
    /// step of a start. Nothing is chosen here; the first selection joins the worker and falls
    /// back to listing on its own thread when there is none or it failed.
    fn start_listing(&mut self) {
        let instance = self.instance.clone();
        self.listing = std::thread::Builder::new()
            .name("gpu-adapters".to_string())
            .spawn(move || pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())))
            .ok();
    }

    /// The adapters of the machine: from the worker the first time when it was started, listed
    /// now otherwise.
    async fn list_adapters(&mut self) -> Vec<wgpu::Adapter> {
        if let Some(Ok(adapters)) = self.listing.take().map(std::thread::JoinHandle::join) {
            return adapters;
        }
        self.instance
            .enumerate_adapters(wgpu::Backends::all())
            .await
    }

    /// The id of a device that can present to `surface` (any, without one), made when there is
    /// none yet.
    async fn device(&mut self, surface: Option<&wgpu::Surface<'_>>) -> Result<usize, String> {
        let existing = match surface {
            Some(s) => self
                .devices
                .iter()
                .position(|d| d.adapter.is_surface_supported(s)),
            None => (!self.devices.is_empty()).then_some(0),
        };
        match existing {
            Some(id) => Ok(id),
            None => self.new_device(surface).await,
        }
    }

    /// Chooses an adapter by the rule and makes a device on it.
    async fn new_device(&mut self, surface: Option<&wgpu::Surface<'_>>) -> Result<usize, String> {
        let adapters = self.list_adapters().await;
        crate::startup::mark(crate::startup::ADAPTERS_LISTED);
        let described: Vec<AdapterDesc> = adapters
            .iter()
            .map(|a| {
                let info = a.get_info();
                AdapterDesc {
                    name: info.name,
                    backend: backend_name(info.backend).to_string(),
                    kind: kind_of(info.device_type),
                    can_present: surface.is_none_or(|s| a.is_surface_supported(s)),
                }
            })
            .collect();
        self.found.clone_from(&described);
        let Some(pick) = gpu_select::choose(&described, &Overrides::from_env()) else {
            return Err(format!(
                "no acceptable graphics adapter (found: {})",
                gpu_select::found_text(&described)
            ));
        };
        let adapter = adapters
            .into_iter()
            .nth(pick.index)
            .ok_or("the chosen graphics adapter is gone")?;
        if pick.forced_software {
            log::warn!(
                "the software graphics adapter {:?} was chosen by WGPU_ADAPTER_NAME: it is not \
                 supported and may crash",
                described[pick.index].name
            );
        }
        let maybe = wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: adapter.features() & maybe,
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("cannot create the graphics device: {e}"))?;
        self.devices.push(DeviceHandle {
            adapter,
            device,
            queue,
        });
        Ok(self.devices.len() - 1)
    }

    /// The surface of `target` with its render targets, configured for device `dev_id`.
    fn render_surface(
        &self,
        dev_id: usize,
        target: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
        scale: f64,
    ) -> Result<RenderSurface<'static>, String> {
        let handle = &self.devices[dev_id];
        let format = target
            .get_capabilities(&handle.adapter)
            .formats
            .into_iter()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                )
            })
            .ok_or("the surface has no supported format")?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            color_space: wgpu::SurfaceColorSpace::Auto,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        let (tw, th) = crate::performance::target_size(width, height, scale);
        let (target_texture, target_view) = targets(tw, th, &handle.device);
        let surface = RenderSurface {
            surface: target,
            config,
            dev_id,
            format,
            target_texture,
            target_view,
            blitter: blitter(&handle.device, format, scale),
        };
        self.configure_surface(&surface);
        Ok(surface)
    }

    fn resize_surface(&self, surface: &mut RenderSurface<'_>, width: u32, height: u32, scale: f64) {
        let (tw, th) = crate::performance::target_size(width, height, scale);
        let (texture, view) = targets(tw, th, &self.devices[surface.dev_id].device);
        surface.target_texture = texture;
        surface.target_view = view;
        surface.config.width = width;
        surface.config.height = height;
        self.configure_surface(surface);
    }

    /// Makes the render target of `surface` the size `scale` gives and the blit filter that
    /// goes with it; the surface itself is not touched.
    fn retarget(&self, surface: &mut RenderSurface<'_>, scale: f64) {
        let device = &self.devices[surface.dev_id].device;
        let (tw, th) =
            crate::performance::target_size(surface.config.width, surface.config.height, scale);
        let (texture, view) = targets(tw, th, device);
        surface.target_texture = texture;
        surface.target_view = view;
        surface.blitter = blitter(device, surface.format, scale);
    }

    fn set_present_mode(&self, surface: &mut RenderSurface<'_>, mode: wgpu::PresentMode) {
        surface.config.present_mode = mode;
        self.configure_surface(surface);
    }

    fn configure_surface(&self, surface: &RenderSurface<'_>) {
        let device = &self.devices[surface.dev_id].device;
        surface.surface.configure(device, &surface.config);
    }
}

/// The blit from a render target to the surface: nearest when the two have the same size (a
/// copy, as it always was), linear when the target was rendered at a smaller `scale`.
fn blitter(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    scale: f64,
) -> wgpu::util::TextureBlitter {
    let filter = if scale < 1.0 {
        wgpu::FilterMode::Linear
    } else {
        wgpu::FilterMode::Nearest
    };
    wgpu::util::TextureBlitterBuilder::new(device, format)
        .sample_type(filter)
        .build()
}

/// The texture the renderer draws into (a compute shader cannot bind the surface texture) and
/// its view; the surface gets a blit of it.
fn targets(width: u32, height: u32, device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        format: wgpu::TextureFormat::Rgba8Unorm,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// The GPU state every window shares: one instance and adapter, one device, and one vello
/// renderer per device (the renderer's pipelines are built once, not once per window).
pub struct GpuContext {
    context: Devices,
    /// Indexed like `context.devices`; created the first time a surface uses the device.
    renderers: Vec<Option<Renderer>>,
    /// What the callbacks of this context's devices recorded.
    health: Arc<GpuHealth>,
    /// How many of `context.devices` have the callbacks.
    watched: usize,
    /// The adapter line was kept for the diagnostics report.
    adapter_noted: bool,
    /// A context built after a loss keeps the adapter line without writing it again (the
    /// recovery line carries it).
    quiet_adapter: bool,
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
    /// The fraction of the surface size the render target has (see [`crate::performance`]).
    scale: f64,
    /// The scene scaled into the smaller target, kept between frames so it does not reallocate.
    scaled: Scene,
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
            context: Devices::new(),
            renderers: Vec::new(),
            health: Arc::new(GpuHealth::default()),
            watched: 0,
            adapter_noted: false,
            quiet_adapter: false,
        }
    }

    /// Starts listing the adapters on a worker (see [`Devices::start_listing`]); the live app
    /// calls it first thing, before the window that needs them exists.
    pub fn start_listing(&mut self) {
        self.context.start_listing();
    }

    /// An empty context for the graphics of an app that lost its device.
    pub fn rebuilt() -> Self {
        Self {
            quiet_adapter: true,
            ..Self::new()
        }
    }

    /// The adapters the last selection saw (none when the system has no graphics adapter at
    /// all), for the start-up message.
    pub fn found(&self) -> &[AdapterDesc] {
        &self.context.found
    }

    /// What the callbacks of this context's devices recorded.
    pub fn health(&self) -> &Arc<GpuHealth> {
        &self.health
    }

    /// Whether a device of this context was lost.
    pub fn is_lost(&self) -> bool {
        self.health.is_lost()
    }

    /// The adapter line of the device in use, once there is one.
    pub fn adapter_line(&self) -> Option<String> {
        let handle = self.context.devices.first()?;
        let info = handle.adapter.get_info();
        let driver = format!("{} {}", info.driver, info.driver_info);
        Some(crate::startup::adapter_line(
            &info.name,
            backend_name(info.backend),
            device_type_name(info.device_type),
            &driver,
        ))
    }

    /// Takes every device away, as a driver reset does (the test switch of the recovery): the
    /// callbacks report it like the real thing.
    pub fn force_loss(&self) {
        for handle in &self.context.devices {
            handle.device.destroy();
            let _ = handle.device.poll(wgpu::PollType::Poll);
        }
        if !self.health.is_lost() {
            self.health.mark_lost("lost on request (test switch)");
        }
    }

    /// Puts the callbacks on the devices made since the last call.
    fn watch_new_devices(&mut self) {
        for handle in self.context.devices.iter().skip(self.watched) {
            self.health.watch(&handle.device);
        }
        self.watched = self.context.devices.len();
    }

    /// The largest side a surface or render target of device `dev_id` may have.
    fn max_side(&self, dev_id: usize) -> u32 {
        self.context
            .devices
            .get(dev_id)
            .map_or(wgpu::Limits::default().max_texture_dimension_2d, |h| {
                h.device.limits().max_texture_dimension_2d
            })
    }

    /// Creates the surface (vsync: `AutoVsync`) for `window`, and the renderer of its device
    /// when no window has needed it yet.
    pub fn create_surface(
        &mut self,
        window: Arc<Window>,
        width: u32,
        height: u32,
        scale: f64,
    ) -> Result<WindowSurface, String> {
        if self.is_lost() {
            return Err("GPU surface: the graphics device is lost".to_string());
        }
        let target = self
            .context
            .instance
            .create_surface(window)
            .map_err(|e| format!("GPU surface: {e}"))?;
        // The device is picked here, before the surface is configured, so the callbacks are on
        // it for everything that follows.
        let dev_id = pollster::block_on(self.context.device(Some(&target)))
            .map_err(|e| format!("GPU surface: {e}"))?;
        crate::startup::mark(crate::startup::DEVICE_CREATED);
        self.watch_new_devices();
        let (width, height) = clamp_size(width, height, self.max_side(dev_id));
        let surface = self
            .context
            .render_surface(
                dev_id,
                target,
                width,
                height,
                wgpu::PresentMode::AutoVsync,
                scale,
            )
            .map_err(|e| format!("GPU surface: {e}"))?;
        crate::startup::mark(crate::startup::ADAPTER_CHOSEN);
        self.note_adapter(surface.dev_id);
        self.renderer_for(surface.dev_id)?;
        crate::startup::mark(crate::startup::RENDERER_READY);
        Ok(WindowSurface {
            surface,
            scale,
            scaled: Scene::new(),
        })
    }

    /// Writes the adapter of device `dev_id` to the log and keeps it for the diagnostics report,
    /// the first time a device is used. Selection is untouched: this only reads the adapter.
    fn note_adapter(&mut self, dev_id: usize) {
        if self.adapter_noted || (!self.quiet_adapter && crate::startup::adapter().is_some()) {
            return;
        }
        self.adapter_noted = true;
        if let Some(handle) = self.context.devices.get(dev_id) {
            let info = handle.adapter.get_info();
            let driver = format!("{} {}", info.driver, info.driver_info);
            let line = crate::startup::adapter_line(
                &info.name,
                backend_name(info.backend),
                device_type_name(info.device_type),
                &driver,
            );
            if self.quiet_adapter {
                crate::startup::keep_adapter(line);
            } else {
                crate::startup::set_adapter(line);
            }
        }
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

    /// Resizes `surface` (physical pixels), to at most the device's texture limit on a side.
    pub fn resize(&mut self, surface: &mut WindowSurface, width: u32, height: u32) {
        let (w, h) = clamp_size(width, height, self.max_side(surface.surface.dev_id));
        let s = &mut surface.surface;
        if (w, h) != (s.config.width, s.config.height) {
            self.context.resize_surface(s, w, h, surface.scale);
        }
    }

    /// Renders `surface` at `scale` of its size from now on (1.0: the full size). A change makes a
    /// new render target; the next [`Self::render`] uses it.
    pub fn set_render_scale(&mut self, surface: &mut WindowSurface, scale: f64) {
        if (surface.scale - scale).abs() > f64::EPSILON {
            surface.scale = scale;
            self.context.retarget(&mut surface.surface, scale);
        }
    }

    /// Renders `scene` at the surface size and presents it.
    pub fn render(
        &mut self,
        surface: &mut WindowSurface,
        scene: &Scene,
        base: Color,
    ) -> FrameResult {
        if self.is_lost() {
            return FrameResult::Lost;
        }
        let WindowSurface {
            surface, scaled, ..
        } = surface;
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
            wgpu::CurrentSurfaceTexture::Lost => {
                self.health.mark_lost("the window surface was lost");
                return FrameResult::Lost;
            }
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
        let (tw, th) = (
            surface.target_texture.width(),
            surface.target_texture.height(),
        );
        // A target smaller than the surface gets the scene scaled to fit it; at the full size the
        // scene is rendered as it is.
        let scene = if (tw, th) == (w, h) {
            scene
        } else {
            scaled.reset();
            scaled.append(
                scene,
                Some(Affine::scale_non_uniform(
                    f64::from(tw) / f64::from(w),
                    f64::from(th) / f64::from(h),
                )),
            );
            &*scaled
        };
        if renderer
            .render_to_texture(
                device,
                queue,
                scene,
                &surface.target_view,
                &params(base, tw, th),
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
    let mut context = Devices::new();
    let id = pollster::block_on(context.device(None))?;
    let handle = &context.devices[id];
    let (device, queue) = (&handle.device, &handle.queue);
    let health = Arc::new(GpuHealth::default());
    health.watch(device);
    let (width, height) = clamp_size(width, height, device.limits().max_texture_dimension_2d);
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
    if health.is_lost() {
        return Err("the graphics device was lost".to_string());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_device_sets_the_flag_and_keeps_the_first_reason() {
        let health = GpuHealth::default();
        assert!(!health.is_lost());
        assert_eq!(health.take_reason(), None);
        health.mark_lost("Unknown: the driver reset");
        health.mark_lost("a later echo");
        assert!(health.is_lost());
        assert_eq!(
            health.take_reason().as_deref(),
            Some("Unknown: the driver reset")
        );
        // Said once; the flag stays until the context is replaced.
        assert_eq!(health.take_reason(), None);
        assert!(health.is_lost());
    }

    #[test]
    fn a_rejected_command_is_noted_once_and_does_not_lose_the_device() {
        let health = GpuHealth::default();
        health.note("bad bind group");
        health.note("bad bind group");
        health.note("bad pipeline");
        assert!(!health.is_lost());
        assert_eq!(health.take_notes(), ["bad bind group", "bad pipeline"]);
        health.note("bad bind group");
        assert!(health.take_notes().is_empty());
    }

    #[test]
    fn the_notes_are_bounded() {
        let health = GpuHealth::default();
        for i in 0..100 {
            health.note(&format!("error {i}"));
        }
        assert_eq!(health.take_notes().len(), MAX_NOTES);
    }

    #[test]
    fn the_adapter_words_name_the_backend_and_the_kind() {
        assert_eq!(backend_name(wgpu::Backend::Dx12), "DX12");
        assert_eq!(backend_name(wgpu::Backend::Vulkan), "Vulkan");
        assert_eq!(backend_name(wgpu::Backend::Metal), "Metal");
        assert_eq!(
            device_type_name(wgpu::DeviceType::IntegratedGpu),
            "integrated"
        );
        assert_eq!(device_type_name(wgpu::DeviceType::DiscreteGpu), "discrete");
        assert_eq!(device_type_name(wgpu::DeviceType::Cpu), "CPU");
        assert_eq!(device_type_name(wgpu::DeviceType::VirtualGpu), "virtual");
    }
}
