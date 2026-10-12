use std::sync::Arc;

use anyhow::{bail, Context};
use vello::{
    kurbo::{Affine, Size},
    peniko::{Color, ImageData},
    util::{DeviceHandle, RenderContext, RenderSurface},
    wgpu, AaConfig, RenderParams, Renderer, RendererOptions, Scene,
};
use winit::{dpi::PhysicalSize, event_loop::ActiveEventLoop, window::Window};

pub const ANTIALIASING: AaConfig = AaConfig::Msaa16;

pub enum Presentation {
    Presented,
    Unavailable,
}

/// Completed stages of a presentation, including stages that reuse cached work.
pub enum Stage {
    Acquire,
    Rasterize,
    Submit,
}

/// Identifies which retained scene layers need rasterization.
pub enum Frame<'a> {
    /// The background and foreground have changed.
    Rebuilt(&'a Scene),
    /// Only the foreground has changed.
    Retained(&'a Scene),
    /// Both scene layers can reuse the cached raster.
    Unchanged(&'a Scene),
}

enum TargetState {
    Dirty,
    Ready,
}

pub struct BackgroundCache {
    image: Option<ImageData>,
    composite: Scene,
}

impl BackgroundCache {
    pub fn new() -> Self {
        Self {
            image: None,
            composite: Scene::new(),
        }
    }

    pub fn compose(
        &mut self,
        renderer: &mut Renderer,
        device: &DeviceHandle,
        background: Frame<'_>,
        foreground: &Scene,
        params: &RenderParams,
    ) -> anyhow::Result<&Scene> {
        let rebuild = matches!(background, Frame::Rebuilt(_))
            || self
                .image
                .as_ref()
                .is_none_or(|image| image.width != params.width || image.height != params.height);
        if rebuild {
            let source = match background {
                Frame::Rebuilt(scene) | Frame::Retained(scene) | Frame::Unchanged(scene) => scene,
            };
            let texture = device.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Cached background"),
                size: wgpu::Extent3d {
                    width: params.width,
                    height: params.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            renderer.render_to_texture(
                &device.device,
                &device.queue,
                source,
                &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                params,
            )?;
            self.invalidate(renderer);
            self.image = Some(renderer.register_texture(texture));
        }
        self.composite.reset();
        self.composite.draw_image(
            self.image
                .as_ref()
                .expect("the background has been rasterized"),
            Affine::IDENTITY,
        );
        self.composite.append(foreground, None);
        Ok(&self.composite)
    }

    fn invalidate(&mut self, renderer: &mut Renderer) {
        if let Some(image) = self.image.take() {
            renderer.unregister_texture(image);
        }
    }
}

pub struct RenderWindow {
    surface: RenderSurface<'static>,
    renderer: Renderer,
    context: RenderContext,
    pub window: Arc<Window>,
    pub scene: Scene,
    background: BackgroundCache,
    target: TargetState,
}

pub fn renderer(device: &wgpu::Device) -> anyhow::Result<Renderer> {
    Ok(Renderer::new(
        device,
        RendererOptions {
            antialiasing_support: [ANTIALIASING].into_iter().collect(),
            ..Default::default()
        },
    )?)
}

impl RenderWindow {
    pub fn device(&self) -> &DeviceHandle {
        &self.context.devices[self.surface.dev_id]
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.surface.format
    }

    /// The current drawable extent, retaining the last size while minimized.
    pub fn viewport(&self) -> Size {
        Size::new(
            f64::from(self.surface.config.width),
            f64::from(self.surface.config.height),
        )
    }

    pub fn new(
        event_loop: &ActiveEventLoop,
        preferences: wgpu::DeviceDescriptor<'_>,
    ) -> anyhow::Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_inner_size(PhysicalSize::new(1280, 440))
                    .with_title("VU"),
            )?,
        );
        let size = window.inner_size();
        let mut context = RenderContext::new();
        let surface = context.instance.create_surface(window.clone())?;
        pollster::block_on(prepare_device(&mut context, Some(&surface), preferences))?;
        // Owning the Arc in the surface gives it a safe 'static lifetime.
        let surface = pollster::block_on(context.create_render_surface(
            surface,
            size.width.max(1),
            size.height.max(1),
            wgpu::PresentMode::AutoVsync,
        ))
        .context("Failed to create the Vello window surface")?;
        let renderer = renderer(&context.devices[surface.dev_id].device)?;
        Ok(Self {
            surface,
            renderer,
            context,
            window,
            scene: Scene::new(),
            background: BackgroundCache::new(),
            target: TargetState::Dirty,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        // Minimized windows have no drawable surface; keep the last nonempty target.
        if size.width > 0 && size.height > 0 {
            self.target = TargetState::Dirty;
            self.context
                .resize_surface(&mut self.surface, size.width, size.height);
        }
    }

    pub fn present(
        &mut self,
        background: Frame<'_>,
        mut completed: impl FnMut(Stage),
        paint: impl FnOnce(
            &DeviceHandle,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
            &wgpu::TextureView,
        ),
    ) -> anyhow::Result<Presentation> {
        use wgpu::CurrentSurfaceTexture;
        if !matches!(background, Frame::Unchanged(_)) {
            self.target = TargetState::Dirty;
        }
        if matches!(background, Frame::Rebuilt(_)) {
            self.background.invalidate(&mut self.renderer);
        }
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(Presentation::Unavailable);
        }
        let frame = self.surface.surface.get_current_texture();
        let reconfigure = matches!(frame, CurrentSurfaceTexture::Suboptimal(_));
        let texture = match frame {
            CurrentSurfaceTexture::Success(texture)
            | CurrentSurfaceTexture::Suboptimal(texture) => texture,
            CurrentSurfaceTexture::Outdated => {
                self.context.configure_surface(&self.surface);
                return Ok(Presentation::Unavailable);
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => {
                return Ok(Presentation::Unavailable)
            }
            state @ (CurrentSurfaceTexture::Lost | CurrentSurfaceTexture::Validation) => {
                bail!("Failed to acquire the Vello surface: {state:?}")
            }
        };
        completed(Stage::Acquire);
        let device = &self.context.devices[self.surface.dev_id];
        let params = RenderParams {
            base_color: Color::TRANSPARENT,
            width: self.surface.config.width,
            height: self.surface.config.height,
            antialiasing_method: ANTIALIASING,
        };
        if matches!(self.target, TargetState::Dirty) {
            let scene = self.background.compose(
                &mut self.renderer,
                device,
                background,
                &self.scene,
                &params,
            )?;
            self.renderer.render_to_texture(
                &device.device,
                &device.queue,
                scene,
                &self.surface.target_view,
                &params,
            )?;
            self.target = TargetState::Ready;
        }
        completed(Stage::Rasterize);
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        paint(device, &mut encoder, &self.surface.target_view, &view);
        device.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        device.queue.present(texture);
        if reconfigure {
            self.context.configure_surface(&self.surface);
        }
        completed(Stage::Submit);
        Ok(Presentation::Presented)
    }
}

/// Negotiate optional capabilities before the context allocates render resources.
pub async fn prepare_device(
    context: &mut RenderContext,
    surface: Option<&wgpu::Surface<'_>>,
    mut preferences: wgpu::DeviceDescriptor<'_>,
) -> anyhow::Result<usize> {
    assert!(
        context.devices.is_empty(),
        "device preferences must precede render resources"
    );
    let id = context
        .device(surface)
        .await
        .context("No compatible graphics device")?;
    let handle = &mut context.devices[id];
    if handle
        .device
        .features()
        .contains(preferences.required_features)
        && preferences
            .required_limits
            .check_limits(&handle.device.limits())
    {
        return Ok(id);
    }
    if handle
        .adapter()
        .features()
        .contains(preferences.required_features)
        && preferences
            .required_limits
            .check_limits(&handle.adapter().limits())
    {
        preferences.required_features |= handle.device.features();
        let (device, queue) = handle.adapter().request_device(&preferences).await?;
        handle.device = device;
        handle.queue = queue;
    }
    Ok(id)
}
