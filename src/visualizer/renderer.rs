use vello::{kurbo::Size, util::DeviceHandle, wgpu};

use super::{Frame, LineInstance, ModelComposite, RibbonInstance};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    viewport: [f32; 4],
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
            label: Some("Visualizer retained background"),
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

/// Composites the retained meter face, swept motion ribbons, and crisp lines.
pub struct Renderer {
    background_pipeline: wgpu::RenderPipeline,
    background_layout: wgpu::BindGroupLayout,
    background: Option<BackgroundBinding>,
    line_pipeline: wgpu::RenderPipeline,
    model_additive_pipeline: wgpu::RenderPipeline,
    model_multiplicative_pipeline: wgpu::RenderPipeline,
    ribbon_pipeline: wgpu::RenderPipeline,
    model_additive_ribbon_pipeline: wgpu::RenderPipeline,
    model_multiplicative_ribbon_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bindings: wgpu::BindGroup,
    uploaded_viewport: Option<[f32; 4]>,
    line_buffer: wgpu::Buffer,
    line_capacity: usize,
    ribbon_buffer: wgpu::Buffer,
    ribbon_capacity: usize,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let background_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Visualizer background"),
            source: wgpu::ShaderSource::Wgsl(include_str!("background.wgsl").into()),
        });
        let background_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Visualizer background"),
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
        let background_layouts = [Some(&background_layout)];
        let background_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Visualizer background"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("Visualizer background"),
                    bind_group_layouts: &background_layouts,
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &background_shader,
                entry_point: Some("background_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &background_shader,
                entry_point: Some("background_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Visualizer uniforms"),
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
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Visualizer uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Visualizer uniforms"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Visualizer lines"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lines.wgsl").into()),
        });
        let line_attributes = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 32,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 48,
                shader_location: 3,
            },
        ];
        let line_pipeline = instance_pipeline(
            device,
            format,
            "Visualizer lines",
            &line_shader,
            "line_vertex",
            "line_fragment",
            &uniform_layout,
            std::mem::size_of::<LineInstance>() as u64,
            &line_attributes,
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            wgpu::ColorWrites::ALL,
        );
        let additive_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let multiplicative_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let model_additive_pipeline = instance_pipeline(
            device,
            format,
            "Visualizer model additive",
            &line_shader,
            "line_vertex",
            "line_additive_fragment",
            &uniform_layout,
            std::mem::size_of::<LineInstance>() as u64,
            &line_attributes,
            additive_blend,
            wgpu::ColorWrites::COLOR,
        );
        let model_multiplicative_pipeline = instance_pipeline(
            device,
            format,
            "Visualizer model multiplicative",
            &line_shader,
            "line_vertex",
            "line_multiply_fragment",
            &uniform_layout,
            std::mem::size_of::<LineInstance>() as u64,
            &line_attributes,
            multiplicative_blend,
            wgpu::ColorWrites::COLOR,
        );

        let ribbon_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Visualizer ribbons"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ribbons.wgsl").into()),
        });
        let ribbon_attributes = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 32,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 48,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 64,
                shader_location: 4,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 80,
                shader_location: 5,
            },
        ];
        let ribbon_pipeline = instance_pipeline(
            device,
            format,
            "Visualizer ribbons",
            &ribbon_shader,
            "ribbon_vertex",
            "ribbon_fragment",
            &uniform_layout,
            std::mem::size_of::<RibbonInstance>() as u64,
            &ribbon_attributes,
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            wgpu::ColorWrites::ALL,
        );
        let model_additive_ribbon_pipeline = instance_pipeline(
            device,
            format,
            "Visualizer model additive ribbons",
            &ribbon_shader,
            "ribbon_vertex",
            "ribbon_additive_fragment",
            &uniform_layout,
            std::mem::size_of::<RibbonInstance>() as u64,
            &ribbon_attributes,
            additive_blend,
            wgpu::ColorWrites::COLOR,
        );
        let model_multiplicative_ribbon_pipeline = instance_pipeline(
            device,
            format,
            "Visualizer model multiplicative ribbons",
            &ribbon_shader,
            "ribbon_vertex",
            "ribbon_multiply_fragment",
            &uniform_layout,
            std::mem::size_of::<RibbonInstance>() as u64,
            &ribbon_attributes,
            multiplicative_blend,
            wgpu::ColorWrites::COLOR,
        );

        let line_capacity = 256;
        let line_buffer = create_instance_buffer::<LineInstance>(device, line_capacity);
        let ribbon_capacity = 256;
        let ribbon_buffer = create_instance_buffer::<RibbonInstance>(device, ribbon_capacity);
        Self {
            background_pipeline,
            background_layout,
            background: None,
            line_pipeline,
            model_additive_pipeline,
            model_multiplicative_pipeline,
            ribbon_pipeline,
            model_additive_ribbon_pipeline,
            model_multiplicative_ribbon_pipeline,
            uniforms,
            uniform_bindings,
            uploaded_viewport: None,
            line_buffer,
            line_capacity,
            ribbon_buffer,
            ribbon_capacity,
        }
    }

    fn ensure_line_capacity(&mut self, device: &wgpu::Device, count: usize) {
        if count <= self.line_capacity {
            return;
        }
        self.line_capacity = count.next_power_of_two();
        self.line_buffer = create_instance_buffer::<LineInstance>(device, self.line_capacity);
    }

    fn ensure_ribbon_capacity(&mut self, device: &wgpu::Device, count: usize) {
        if count <= self.ribbon_capacity {
            return;
        }
        self.ribbon_capacity = count.next_power_of_two();
        self.ribbon_buffer = create_instance_buffer::<RibbonInstance>(device, self.ribbon_capacity);
    }

    pub fn draw(
        &mut self,
        device: &DeviceHandle,
        encoder: &mut wgpu::CommandEncoder,
        background: &wgpu::TextureView,
        target: &wgpu::TextureView,
        viewport: Size,
        frame: Frame<'_>,
    ) {
        if self
            .background
            .as_ref()
            .is_none_or(|binding| binding.source != *background)
        {
            self.background = Some(BackgroundBinding::new(
                &device.device,
                &self.background_layout,
                background,
            ));
        }
        let viewport = [viewport.width as f32, viewport.height as f32, 0.0, 0.0];
        if self.uploaded_viewport != Some(viewport) {
            device.queue.write_buffer(
                &self.uniforms,
                0,
                bytemuck::bytes_of(&Uniforms { viewport }),
            );
            self.uploaded_viewport = Some(viewport);
        }
        let model_ribbon_count = frame.model_ribbons.len();
        let ribbon_count = frame.ribbons.len();
        let total_ribbons = model_ribbon_count + ribbon_count;
        let ribbon_stride = std::mem::size_of::<RibbonInstance>() as u64;
        let model_ribbon_bytes = model_ribbon_count as u64 * ribbon_stride;
        if total_ribbons > 0 {
            self.ensure_ribbon_capacity(&device.device, total_ribbons);
            if model_ribbon_count > 0 {
                device.queue.write_buffer(
                    &self.ribbon_buffer,
                    0,
                    bytemuck::cast_slice(frame.model_ribbons),
                );
            }
            if ribbon_count > 0 {
                device.queue.write_buffer(
                    &self.ribbon_buffer,
                    model_ribbon_bytes,
                    bytemuck::cast_slice(frame.ribbons),
                );
            }
        }
        let model_count = frame.model_lines.len();
        let line_count = frame.lines.len();
        let total_lines = model_count + line_count;
        let line_stride = std::mem::size_of::<LineInstance>() as u64;
        let model_bytes = model_count as u64 * line_stride;
        if total_lines > 0 {
            self.ensure_line_capacity(&device.device, total_lines);
            if model_count > 0 {
                device.queue.write_buffer(
                    &self.line_buffer,
                    0,
                    bytemuck::cast_slice(frame.model_lines),
                );
            }
            if line_count > 0 {
                device.queue.write_buffer(
                    &self.line_buffer,
                    model_bytes,
                    bytemuck::cast_slice(frame.lines),
                );
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Visualizer overlay"),
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
        pass.set_bind_group(
            0,
            &self
                .background
                .as_ref()
                .expect("the retained background has been bound")
                .bindings,
            &[],
        );
        pass.draw(0..3, 0..1);
        if model_count > 0 {
            let pipeline = match frame.model_composite {
                ModelComposite::Additive => &self.model_additive_pipeline,
                ModelComposite::Multiplicative => &self.model_multiplicative_pipeline,
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.uniform_bindings, &[]);
            pass.set_vertex_buffer(0, self.line_buffer.slice(0..model_bytes));
            pass.draw(0..6, 0..model_count as u32);
        }
        if model_ribbon_count > 0 {
            let pipeline = match frame.model_composite {
                ModelComposite::Additive => &self.model_additive_ribbon_pipeline,
                ModelComposite::Multiplicative => &self.model_multiplicative_ribbon_pipeline,
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.uniform_bindings, &[]);
            pass.set_vertex_buffer(0, self.ribbon_buffer.slice(0..model_ribbon_bytes));
            pass.draw(0..6, 0..model_ribbon_count as u32);
        }
        if ribbon_count > 0 {
            pass.set_pipeline(&self.ribbon_pipeline);
            pass.set_bind_group(0, &self.uniform_bindings, &[]);
            pass.set_vertex_buffer(0, self.ribbon_buffer.slice(model_ribbon_bytes..));
            pass.draw(0..6, 0..ribbon_count as u32);
        }
        if line_count > 0 {
            pass.set_pipeline(&self.line_pipeline);
            pass.set_bind_group(0, &self.uniform_bindings, &[]);
            pass.set_vertex_buffer(0, self.line_buffer.slice(model_bytes..));
            pass.draw(0..6, 0..line_count as u32);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn instance_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
    shader: &wgpu::ShaderModule,
    vertex_entry: &str,
    fragment_entry: &str,
    uniform_layout: &wgpu::BindGroupLayout,
    stride: u64,
    attributes: &[wgpu::VertexAttribute],
    blend: wgpu::BlendState,
    write_mask: wgpu::ColorWrites,
) -> wgpu::RenderPipeline {
    let layouts = [Some(uniform_layout)];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &layouts,
                immediate_size: 0,
            }),
        ),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex_entry),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: stride,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_instance_buffer<T>(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Visualizer instance buffer"),
        size: capacity as u64 * std::mem::size_of::<T>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::BufWriter, sync::mpsc};

    use vello::{kurbo::Size, util::RenderContext, wgpu};

    use super::*;
    use crate::{
        audio::timeline::{AudioInterval, AudioTime},
        config::ShutterFrames,
        theme::Theme,
        visualizer::Visualizer,
    };

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_wireframe_lines_and_swept_ribbons() -> anyhow::Result<()> {
        let mut context = RenderContext::new();
        let id = pollster::block_on(crate::render::prepare_device(
            &mut context,
            None,
            wgpu::DeviceDescriptor::default(),
        ))?;
        let device = &context.devices[id];
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let width = 1280;
        let height = 440;
        let background = device.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Visualizer test background"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let colors = Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = crate::visualizer::Palette::from_theme(crate::theme::ThemeId::default());
        let background_pixels: Vec<u8> = (0..width * height)
            .flat_map(|_| {
                colors.background.components[..3]
                    .iter()
                    .map(|value| (value * 255.0) as u8)
                    .chain(std::iter::once(255))
            })
            .collect();
        device.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &background,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &background_pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let target = device.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Visualizer test target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let background_view = background.create_view(&wgpu::TextureViewDescriptor::default());
        let mut renderer = Renderer::new(&device.device, format);
        let mut visualizer = Visualizer::new();
        visualizer.set_samplerate(48_000);
        let mut at = AudioTime::ZERO;
        for frame in 0..180 {
            let mut input = Vec::with_capacity(800);
            for index in 0..800 {
                let sample = frame * 800 + index;
                let time = sample as f32 / 48_000.0;
                let kick = if (0..10).contains(&(sample % 9_600)) {
                    0.9
                } else {
                    0.0
                };
                let tone = 0.25 * (std::f32::consts::TAU * 1_000.0 * time).sin()
                    + 0.16 * (std::f32::consts::TAU * 220.0 * time).sin();
                let kick_tone = kick * (std::f32::consts::TAU * 64.0 * time).sin();
                input.push([tone + kick_tone; 2]);
            }
            visualizer.consume_buf(&input, 1.0);
            let end = at.shifted(800.0 / 48_000.0);
            visualizer.update(
                AudioInterval::new(at, end),
                ShutterFrames::try_from(8.0)?,
                800.0 / 48_000.0,
                &palette,
                (width as f32, height as f32),
                1.0,
                1.0,
            );
            at = end;
        }
        let frame = visualizer.frame();
        assert!(frame.lines.len() + frame.ribbons.len() >= 12);
        assert!(
            !frame.ribbons.is_empty(),
            "motion history must produce swept ribbons"
        );
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for line in frame.lines {
            for (x, y) in [
                (line.endpoints[0], line.endpoints[1]),
                (line.endpoints[2], line.endpoints[3]),
            ] {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
        for ribbon in frame.ribbons {
            for corner in [ribbon.start, ribbon.end] {
                for (x, y) in [(corner[0], corner[1]), (corner[2], corner[3])] {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(
            min_x > -(width as f32) / 4.0
                && max_x < width as f32 * 1.25
                && min_y > -(height as f32) / 4.0
                && max_y < height as f32 * 1.25,
            "the projected scene ran away from the viewport: {min_x}..{max_x}, {min_y}..{max_y}"
        );
        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        renderer.draw(
            device,
            &mut encoder,
            &background_view,
            &target_view,
            Size::new(f64::from(width), f64::from(height)),
            frame,
        );
        let row_bytes = width * 4;
        let readback = device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Visualizer test readback"),
            size: u64::from(row_bytes * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
            },
            target.size(),
        );
        device.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let pixels = readback.slice(..).get_mapped_range()?.to_vec();
        readback.unmap();
        let ink = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(background_pixels.as_chunks::<4>().0)
            .filter(|(pixel, background)| {
                pixel[..3]
                    .iter()
                    .zip(&background[..3])
                    .any(|(pixel, background)| pixel.abs_diff(*background) > 24)
            })
            .count();
        eprintln!(
            "visualizer ink pixels: {ink}; {} lines, {} ribbons",
            frame.lines.len(),
            frame.ribbons.len()
        );
        assert!(ink > 2_000, "the wireframe must cover the retained face");
        if let Some(path) = std::env::var_os("VU_RENDER_PREVIEW_PATH") {
            let path = std::path::PathBuf::from(path).with_extension("visualizer.png");
            let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&pixels)?;
        }
        Ok(())
    }
}
