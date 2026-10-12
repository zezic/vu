use vello::{
    kurbo::{Point, Rect, Size},
    util::DeviceHandle,
    wgpu,
};

use super::{
    NeedleAppearance, GLOW_RADIUS, LAMP_CENTER, LAMP_RADIUS, NEEDLE_INK, NEEDLE_RADIUS,
    NEEDLE_WIDTH,
};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuNeedle {
    /// Inverse transform's x row, followed by pixels per meter unit.
    inverse_x: [f32; 4],
    /// Inverse transform's y row, padded to WGSL's vec4 alignment.
    inverse_y: [f32; 4],
    needle_bounds: [f32; 4],
    glow_bounds: [f32; 4],
    /// Exposure mean, minimum and maximum angles, followed by wing opacity.
    angles: [f32; 4],
    /// Radius, stroke width, radial opacity peak and overload strength.
    shape: [f32; 4],
}

impl From<&NeedleAppearance> for GpuNeedle {
    fn from(needle: &NeedleAppearance) -> Self {
        let [a, b, c, d, e, f] = needle.transform.inverse().as_coeffs();
        let [x, y, ..] = needle.transform.as_coeffs();
        let scale = x.hypot(y);
        let origin = needle.transform * Point::ORIGIN;
        let bounds = needle
            .screen_edges()
            .into_iter()
            .fold(
                Rect::from_points(origin, needle.screen_tip()),
                |bounds, point| bounds.union_pt(point),
            )
            .inflate(
                NEEDLE_WIDTH * scale / 2.0 + 1.0,
                NEEDLE_WIDTH * scale / 2.0 + 1.0,
            );
        let glow = needle
            .transform
            .transform_rect_bbox(Rect::from_center_size(
                LAMP_CENTER,
                Size::new(GLOW_RADIUS * 2.0, GLOW_RADIUS * 2.0),
            ))
            .inflate(1.0, 1.0);
        Self {
            inverse_x: [a as f32, c as f32, e as f32, scale as f32],
            inverse_y: [b as f32, d as f32, f as f32, 0.0],
            needle_bounds: [
                bounds.x0 as f32,
                bounds.y0 as f32,
                bounds.x1 as f32,
                bounds.y1 as f32,
            ],
            glow_bounds: [
                glow.x0 as f32,
                glow.y0 as f32,
                glow.x1 as f32,
                glow.y1 as f32,
            ],
            angles: [
                needle.angle as f32,
                needle.edges[0] as f32,
                needle.edges[1] as f32,
                needle.blur,
            ],
            shape: [
                NEEDLE_RADIUS as f32,
                NEEDLE_WIDTH as f32,
                needle.peak,
                needle.overload,
            ],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    viewport: [f32; 4],
    ink: [f32; 4],
    light: [f32; 4],
    needles: [GpuNeedle; 2],
}

/// Combines the retained face and animated overlay in one render pass.
pub struct Renderer {
    binding: UniformBinding,
    needle: wgpu::RenderPipeline,
    glow: wgpu::RenderPipeline,
    background_pipeline: wgpu::RenderPipeline,
    background_layout: wgpu::BindGroupLayout,
    background: Option<BackgroundBinding>,
}

enum UniformBinding {
    Immediate,
    Buffered(Box<UniformBuffer>),
}

struct UniformBuffer {
    buffer: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    bindings: wgpu::BindGroup,
    uploaded: Option<Uniforms>,
}

impl UniformBinding {
    fn new(device: &wgpu::Device) -> Self {
        if device.features().contains(wgpu::Features::IMMEDIATES)
            && device.limits().max_immediate_size >= std::mem::size_of::<Uniforms>() as u32
        {
            return Self::Immediate;
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Meter overlay uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Meter overlay bindings"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Meter overlay uniforms"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self::Buffered(Box::new(UniformBuffer {
            buffer,
            layout,
            bindings,
            uploaded: None,
        }))
    }

    fn layout(&self) -> Option<&wgpu::BindGroupLayout> {
        match self {
            Self::Immediate => None,
            Self::Buffered(buffer) => Some(&buffer.layout),
        }
    }

    fn immediate_size(&self) -> u32 {
        match self {
            Self::Immediate => std::mem::size_of::<Uniforms>() as u32,
            Self::Buffered(_) => 0,
        }
    }

    fn declaration(&self) -> &'static str {
        match self {
            Self::Immediate => "var<immediate> data: Uniforms;",
            Self::Buffered(_) => "@group(0) @binding(0) var<uniform> data: Uniforms;",
        }
    }

    fn bind(&mut self, pass: &mut wgpu::RenderPass<'_>, queue: &wgpu::Queue, uniforms: Uniforms) {
        match self {
            Self::Immediate => pass.set_immediates(0, bytemuck::bytes_of(&uniforms)),
            Self::Buffered(buffer) => {
                if buffer.uploaded != Some(uniforms) {
                    queue.write_buffer(&buffer.buffer, 0, bytemuck::bytes_of(&uniforms));
                    buffer.uploaded = Some(uniforms);
                }
                pass.set_bind_group(0, &buffer.bindings, &[]);
            }
        }
    }
}

struct BackgroundBinding {
    source: wgpu::TextureView,
    bindings: wgpu::BindGroup,
}

impl BackgroundBinding {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        source: &wgpu::TextureView,
    ) -> Self {
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Meter retained face"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source),
            }],
        });
        Self {
            source: source.clone(),
            bindings,
        }
    }
}

impl Renderer {
    pub fn device_preferences() -> wgpu::DeviceDescriptor<'static> {
        wgpu::DeviceDescriptor {
            required_features: wgpu::Features::IMMEDIATES,
            required_limits: wgpu::Limits {
                max_immediate_size: std::mem::size_of::<Uniforms>() as u32,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let binding = UniformBinding::new(device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Meter overlay"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    binding.declaration(),
                    include_str!("overlay.wgsl")
                )
                .into(),
            ),
        });
        let background_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Meter retained face"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Meter overlay"),
            bind_group_layouts: &[binding.layout(), Some(&background_layout)],
            immediate_size: binding.immediate_size(),
        });
        let pipeline = |vertex, fragment, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fragment),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            binding,
            needle: pipeline(
                "needle_vertex",
                "needle_fragment",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            glow: pipeline(
                "glow_vertex",
                "glow_fragment",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            background_pipeline: pipeline("background_vertex", "background_fragment", None),
            background_layout,
            background: None,
        }
    }

    pub fn draw(
        &mut self,
        device: &DeviceHandle,
        encoder: &mut wgpu::CommandEncoder,
        background: &wgpu::TextureView,
        target: &wgpu::TextureView,
        viewport: Size,
        needles: &[NeedleAppearance; 2],
    ) {
        if self
            .background
            .as_ref()
            .is_none_or(|bound| bound.source != *background)
        {
            self.background = Some(BackgroundBinding::new(
                &device.device,
                &self.background_layout,
                background,
            ));
        }
        let uniforms = Uniforms {
            viewport: [viewport.width as f32, viewport.height as f32, 0.0, 0.0],
            ink: NEEDLE_INK.components,
            light: [
                LAMP_CENTER.x as f32,
                LAMP_CENTER.y as f32,
                LAMP_RADIUS as f32,
                GLOW_RADIUS as f32,
            ],
            needles: needles.each_ref().map(GpuNeedle::from),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Meter overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.background_pipeline);
        self.binding.bind(&mut pass, &device.queue, uniforms);
        pass.set_bind_group(
            1,
            &self
                .background
                .as_ref()
                .expect("the retained face has been bound")
                .bindings,
            &[],
        );
        pass.draw(0..3, 0..1);
        if needles.iter().any(|needle| needle.overload > 0.0) {
            pass.set_pipeline(&self.glow);
            pass.draw(0..6, 0..2);
        }
        pass.set_pipeline(&self.needle);
        pass.draw(0..6, 0..2);
    }
}
