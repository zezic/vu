mod raster_asset;

pub use raster_asset::{RasterAsset, RasterAssetCache};

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

/// Most plate edges the coverage shader clips against.
/// Keep in sync with the `edges` array in `visualizer/plates.wgsl`.
pub const MAX_PLATE_EDGES: usize = 8;

/// Most contrast plates one frame can blur on the GPU.
pub const MAX_PLATE_INSTANCES: usize = 8;

/// Coverage-mask texels per screen pixel.
pub const PLATE_MASK_SCALE: f64 = 0.5;

/// One moving contrast plate for the coverage shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PlateInstance {
    /// Screen bounds of the swept support, including one pixel of AA padding.
    pub bounds: [f32; 4],
    /// Screen displacement covered by the exposure.
    pub motion: [f32; 2],
    /// Number of edge entries in `edges`.
    pub edge_count: u32,
    pub _padding: u32,
    /// Edge normal xy and support offset, in screen space.
    pub edges: [[f32; 4]; MAX_PLATE_EDGES],
}

/// The GPU coverage mask a scene can sample as a plate brush.
#[derive(Clone)]
pub struct PlateMaskImage {
    pub image: ImageData,
    /// Mask texels per screen pixel.
    pub scale: f64,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PlateUniforms {
    /// Screen width, screen height, mask scale, padding.
    viewport: [f32; 4],
    ink: [f32; 4],
}

/// Renders the exact 2D exposure coverage of all moving plates.
pub(crate) struct PlateMask {
    view: wgpu::TextureView,
    image: ImageData,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    plates: wgpu::Buffer,
    screen: [f32; 2],
    scale: f64,
}

impl PlateMask {
    pub(crate) fn new(
        device: &wgpu::Device,
        renderer: &mut Renderer,
        width: u32,
        height: u32,
    ) -> Self {
        let scale = PLATE_MASK_SCALE;
        let mask_width = ((f64::from(width) * scale).ceil() as u32).max(1);
        let mask_height = ((f64::from(height) * scale).ceil() as u32).max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Plate coverage mask"),
            size: wgpu::Extent3d {
                width: mask_width,
                height: mask_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let image = renderer.register_texture(texture);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Plate coverage"),
            source: wgpu::ShaderSource::Wgsl(include_str!("visualizer/plates.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Plate coverage"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<PlateUniforms>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<PlateInstance>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Plate coverage uniforms"),
            size: std::mem::size_of::<PlateUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let plates = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Plate coverage instances"),
            size: (MAX_PLATE_INSTANCES * std::mem::size_of::<PlateInstance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Plate coverage"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: plates.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Plate coverage"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Max,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Max,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Plate coverage"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("plate_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("plate_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            view,
            image,
            pipeline,
            bind_group,
            uniforms,
            plates,
            screen: [width as f32, height as f32],
            scale,
        }
    }

    pub(crate) fn image(&self) -> PlateMaskImage {
        PlateMaskImage {
            image: self.image.clone(),
            scale: self.scale,
        }
    }

    pub(crate) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        plates: &[PlateInstance],
        ink: Color,
    ) {
        let [red, green, blue, _] = ink.components;
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&PlateUniforms {
                viewport: [self.screen[0], self.screen[1], self.scale as f32, 0.0],
                ink: [red, green, blue, 1.0],
            }),
        );
        let count = plates.len().min(MAX_PLATE_INSTANCES);
        if count > 0 {
            queue.write_buffer(&self.plates, 0, bytemuck::cast_slice(&plates[..count]));
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Plate coverage"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Plate coverage"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.draw(0..6, 0..count as u32);
            }
        }
        queue.submit([encoder.finish()]);
    }
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
            let texture = raster_asset::rasterize_scene(renderer, device, source, params)?;
            self.invalidate(renderer);
            self.image = Some(renderer.register_texture(texture));
        }
        let image = self
            .image
            .as_ref()
            .expect("the background has been rasterized");
        // Solid-only renders can replace Vello's image atlas between uses.
        renderer.mark_override_image_dirty(image);
        self.composite.reset();
        self.composite.draw_image(image, Affine::IDENTITY);
        Ok(&self.composite)
    }

    fn invalidate(&mut self, renderer: &mut Renderer) {
        if let Some(image) = self.image.take() {
            renderer.unregister_texture(image);
        }
    }
}

/// The animated foreground, rasterized with a transparent base so the GPU pass
/// can layer it over the model wireframe.
pub struct ForegroundCache {
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl ForegroundCache {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Animated foreground"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            view,
            width,
            height,
        }
    }

    fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }
}

pub struct RenderWindow {
    surface: RenderSurface<'static>,
    renderer: Renderer,
    context: RenderContext,
    pub window: Arc<Window>,
    pub scene: Scene,
    background: BackgroundCache,
    raster_assets: RasterAssetCache,
    foreground: Option<ForegroundCache>,
    plate_mask: PlateMask,
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
                    .with_inner_size(PhysicalSize::new(1920, 1080))
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
        let device_id = surface.dev_id;
        let mut renderer = renderer(&context.devices[device_id].device)?;
        let plate_mask = PlateMask::new(
            &context.devices[device_id].device,
            &mut renderer,
            size.width.max(1),
            size.height.max(1),
        );
        Ok(Self {
            surface,
            renderer,
            context,
            window,
            scene: Scene::new(),
            background: BackgroundCache::new(),
            raster_assets: RasterAssetCache::new(),
            foreground: None,
            plate_mask,
            target: TargetState::Dirty,
        })
    }

    pub fn plate_mask_image(&self) -> PlateMaskImage {
        self.plate_mask.image()
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        // Minimized windows have no drawable surface; keep the last nonempty target.
        if size.width > 0 && size.height > 0 {
            self.target = TargetState::Dirty;
            self.context
                .resize_surface(&mut self.surface, size.width, size.height);
            let device_id = self.surface.dev_id;
            let plate_mask = PlateMask::new(
                &self.context.devices[device_id].device,
                &mut self.renderer,
                size.width,
                size.height,
            );
            let previous = std::mem::replace(&mut self.plate_mask, plate_mask);
            self.renderer.unregister_texture(previous.image);
        }
    }

    /// Renders the exact exposure coverage of the moving plates into the mask
    /// texture that the foreground scene samples as a plate brush.
    fn update_plate_mask(&mut self, plates: &[PlateInstance], ink: Color) {
        if plates.is_empty() {
            return;
        }
        let device = &self.context.devices[self.surface.dev_id];
        self.plate_mask
            .render(&device.device, &device.queue, plates, ink);
        self.renderer
            .mark_override_image_dirty(&self.plate_mask.image);
    }

    pub fn present(
        &mut self,
        background: Frame<'_>,
        foreground: bool,
        raster_assets: &[RasterAsset],
        plates: &[PlateInstance],
        plate_ink: Color,
        mut completed: impl FnMut(Stage),
        paint: impl FnOnce(
            &DeviceHandle,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
            &wgpu::TextureView,
            Option<&wgpu::TextureView>,
        ),
    ) -> anyhow::Result<Presentation> {
        use wgpu::CurrentSurfaceTexture;
        if matches!(background, Frame::Rebuilt(_)) {
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
        self.update_plate_mask(plates, plate_ink);
        let device = &self.context.devices[self.surface.dev_id];
        self.raster_assets
            .prepare(&mut self.renderer, device, raster_assets)?;
        let params = RenderParams {
            base_color: Color::TRANSPARENT,
            width: self.surface.config.width,
            height: self.surface.config.height,
            antialiasing_method: ANTIALIASING,
        };
        if matches!(self.target, TargetState::Dirty) {
            let scene = self
                .background
                .compose(&mut self.renderer, device, background, &params)?;
            self.renderer.render_to_texture(
                &device.device,
                &device.queue,
                scene,
                &self.surface.target_view,
                &params,
            )?;
            self.target = TargetState::Ready;
        }
        let overlay = if foreground {
            if !self
                .foreground
                .as_ref()
                .is_some_and(|cache| cache.matches(params.width, params.height))
            {
                self.foreground = Some(ForegroundCache::new(
                    &device.device,
                    params.width,
                    params.height,
                ));
            }
            let overlay = self
                .foreground
                .as_mut()
                .expect("the foreground cache has been created");
            let overlay_params = RenderParams {
                base_color: Color::TRANSPARENT,
                width: params.width,
                height: params.height,
                antialiasing_method: ANTIALIASING,
            };
            self.renderer.render_to_texture(
                &device.device,
                &device.queue,
                &self.scene,
                &overlay.view,
                &overlay_params,
            )?;
            Some(&overlay.view)
        } else {
            None
        };
        completed(Stage::Rasterize);
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        paint(
            device,
            &mut encoder,
            &self.surface.target_view,
            &view,
            overlay,
        );
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

impl Default for BackgroundCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plate_instance_matches_the_contour_shader_layout() {
        // Keep in sync with the `Plate` struct in `visualizer/plates.wgsl`.
        assert_eq!(std::mem::size_of::<PlateInstance>(), 160);
        assert_eq!(std::mem::align_of::<PlateInstance>(), 4);
    }
}
