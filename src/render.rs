use std::sync::Arc;

use anyhow::{bail, Context};
use vello::{
    kurbo::Size,
    peniko::Color,
    util::{RenderContext, RenderSurface},
    wgpu, AaConfig, RenderParams, Renderer, RendererOptions, Scene,
};
use winit::{dpi::PhysicalSize, event_loop::ActiveEventLoop, window::Window};

pub const BACKGROUND: Color = Color::from_rgb8(40, 36, 36);

pub enum Presentation {
    Presented,
    Unavailable,
}

pub struct RenderWindow {
    surface: RenderSurface<'static>,
    renderer: Renderer,
    context: RenderContext,
    pub window: Arc<Window>,
    pub scene: Scene,
}

pub fn renderer(device: &wgpu::Device) -> anyhow::Result<Renderer> {
    Ok(Renderer::new(
        device,
        RendererOptions {
            antialiasing_support: [AaConfig::Msaa16].into_iter().collect(),
            ..Default::default()
        },
    )?)
}

impl RenderWindow {
    /// The current drawable extent, retaining the last size while minimized.
    pub fn viewport(&self) -> Size {
        Size::new(
            f64::from(self.surface.config.width),
            f64::from(self.surface.config.height),
        )
    }

    pub fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_inner_size(PhysicalSize::new(1280, 440))
                    .with_title("VU"),
            )?,
        );
        let size = window.inner_size();
        let mut context = RenderContext::new();
        // Owning the Arc in the surface gives it a safe 'static lifetime.
        let surface = pollster::block_on(context.create_surface(
            window.clone(),
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
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        // Minimized windows have no drawable surface; keep the last nonempty target.
        if size.width > 0 && size.height > 0 {
            self.context
                .resize_surface(&mut self.surface, size.width, size.height);
        }
    }

    pub fn present(&mut self) -> anyhow::Result<Presentation> {
        use wgpu::CurrentSurfaceTexture;
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
        let device = &self.context.devices[self.surface.dev_id];
        self.renderer.render_to_texture(
            &device.device,
            &device.queue,
            &self.scene,
            &self.surface.target_view,
            &RenderParams {
                base_color: BACKGROUND,
                width: self.surface.config.width,
                height: self.surface.config.height,
                antialiasing_method: AaConfig::Msaa16,
            },
        )?;
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.surface.blitter.copy(
            &device.device,
            &mut encoder,
            &self.surface.target_view,
            &view,
        );
        device.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        device.queue.present(texture);
        if reconfigure {
            self.context.configure_surface(&self.surface);
        }
        Ok(Presentation::Presented)
    }
}
