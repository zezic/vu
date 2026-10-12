use std::{fs::File, io::BufWriter, sync::mpsc};

use vello::{
    kurbo::{Point, Rect, Size},
    util::{DeviceHandle, RenderContext},
    wgpu, AaConfig, RenderParams, Renderer, Scene,
};

use super::{Layout, Meter, View};
use crate::{
    config::PreampDb,
    frame_schedule::FrameRateLimit,
    render::{renderer, BackgroundCache, BACKGROUND},
    AudioEvent,
};

#[test]
fn fits_and_centers_both_layouts_in_wide_tall_and_small_windows() {
    for (viewport, expected) in [
        (Size::new(1280.0, 440.0), Layout::Horizontal),
        (Size::new(1600.0, 400.0), Layout::Horizontal),
        (Size::new(900.0, 600.0), Layout::Horizontal),
        (Size::new(320.0, 220.0), Layout::Horizontal),
        (Size::new(800.0, 600.0), Layout::Vertical),
        (Size::new(400.0, 900.0), Layout::Vertical),
        (Size::new(1.0, 1.0), Layout::Vertical),
    ] {
        let view = View::fit(viewport);
        assert_eq!(view.layout, expected);
        let frame = Rect::from_origin_size(Point::ORIGIN, view.layout.size());
        let fitted = view.transform.transform_rect_bbox(frame);
        let center = view.transform * frame.center();
        let epsilon = 1e-9;
        assert!((center.x - viewport.width / 2.0).abs() < epsilon);
        assert!((center.y - viewport.height / 2.0).abs() < epsilon);
        assert!(fitted.x0 >= -epsilon && fitted.y0 >= -epsilon);
        assert!(fitted.x1 <= viewport.width + epsilon && fitted.y1 <= viewport.height + epsilon);
        assert!(
            (fitted.width() / fitted.height() - frame.width() / frame.height()).abs() < epsilon
        );
        assert!(
            (fitted.width() - viewport.width).abs() < epsilon
                || (fitted.height() - viewport.height).abs() < epsilon
        );
        let [left, right] = view.layout.pivots();
        match expected {
            Layout::Horizontal => {
                assert!(left.x < right.x);
                assert_eq!(left.y, right.y);
                let [preamp, limit] = view.layout.readouts();
                assert_eq!(preamp.x, limit.x);
                assert!(preamp.y < limit.y);
            }
            Layout::Vertical => {
                assert!(left.y < right.y);
                assert_eq!(left.x, right.x);
                let [preamp, limit] = view.layout.readouts();
                assert!(preamp.x < limit.x);
                assert_eq!(preamp.y, limit.y);
                assert_eq!(preamp.y + 3.0, view.layout.size().height / 2.0);
            }
        }
    }
}

#[test]
#[ignore = "requires a compute-capable GPU"]
fn renders_stereo_scales_text_needles_and_overload() -> anyhow::Result<()> {
    let mut meter = Meter::new(PreampDb::try_from(0.0)?)?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    // Fill the RMS window, then deliver a buffer whose first frame has a full window.
    for _ in 0..2 {
        meter.audio_event(AudioEvent::Buffer {
            buf: vec![[0.1, 1.0]; 14400],
        });
    }
    let mut context = RenderContext::new();
    let id = pollster::block_on(context.device(None)).expect("a compute-capable GPU is required");
    let device = &context.devices[id];
    let mut renderer = renderer(&device.device)?;
    let mut background = BackgroundCache::new();
    let mut scene = Scene::new();

    for (width, height, limit) in [
        (1280, 440, FrameRateLimit::Fps55),
        (640, 960, FrameRateLimit::VSync),
    ] {
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        for _ in 0..60 {
            scene.reset();
            meter.draw(&mut scene, &view, limit, None, 1.0 / 60.0);
        }
        let params = RenderParams {
            base_color: BACKGROUND,
            width,
            height,
            antialiasing_method: AaConfig::Msaa16,
        };
        let composite = background.compose(
            &mut renderer,
            device,
            crate::render::Frame::Retained(&meter.face),
            &scene,
            &params,
        )?;
        let pixels = render_frame(device, &mut renderer, composite, width, height)?;
        let pixel = |point: Point| {
            let (x, y) = (point.x as usize, point.y as usize);
            &pixels[(y * width as usize + x) * 4..][..4]
        };
        let count_bright = |region: Rect| {
            let region = view.transform.transform_rect_bbox(region);
            (region.y0 as usize..region.y1 as usize)
                .flat_map(|y| {
                    (region.x0 as usize..region.x1 as usize)
                        .map(move |x| Point::new(x as f64, y as f64))
                })
                .filter(|&point| {
                    let p = pixel(point);
                    p[0] > 170 && p[1] > 170 && p[2] > 170
                })
                .count()
        };
        let controls_ink = |region: Rect| {
            let region = view.transform.transform_rect_bbox(region);
            (region.y0 as usize..region.y1 as usize)
                .flat_map(|y| {
                    (region.x0 as usize..region.x1 as usize)
                        .map(move |x| Point::new(x as f64, y as f64))
                })
                .filter(|&point| {
                    let p = pixel(point);
                    p[0] >= 65 && p[0] <= 110 && p[1] >= 55
                })
                .count()
        };
        for (origin, value) in view
            .layout
            .readouts()
            .into_iter()
            .zip([&meter.preamp_value, meter.frame_values.get(&limit).unwrap()])
        {
            let half_width = value.width() / 2.0;
            assert!(
                controls_ink(Rect::new(
                    origin.x - half_width,
                    origin.y + 1.0,
                    origin.x + half_width,
                    origin.y + 13.0
                )) > 40,
                "both setting values must render"
            );
            let hint_offset = super::Readout::HINT_OFFSET;
            for x in [origin.x - hint_offset, origin.x + hint_offset] {
                assert!(
                    controls_ink(Rect::new(x - 9.0, origin.y + 2.0, x + 9.0, origin.y + 13.0)) > 20,
                    "the setting value must have a visible hotkey hint on either side"
                );
            }
        }
        for (channel, pivot) in view.layout.pivots().into_iter().enumerate() {
            let lamp = view.transform * (pivot + (0.0, -82.8));
            if channel == 0 {
                assert!(
                    pixel(lamp)[0] < 50,
                    "the left channel's overload lamp stays dark"
                );
            } else {
                assert!(
                    pixel(lamp)[0] > 200,
                    "the right channel's overload lamp lights up"
                );
            }
            let origin = pivot - (160.0, 207.0);
            let scale_region = Rect::new(
                origin.x,
                origin.y + 10.0,
                origin.x + 320.0,
                origin.y + 100.0,
            );
            assert!(
                count_bright(scale_region) > 400,
                "both scales must render in either layout"
            );
            let label_region =
                Rect::new(origin.x, origin.y + 7.5, origin.x + 320.0, origin.y + 27.5);
            assert!(
                count_bright(label_region) > 50,
                "the DIN labels above each arc must render"
            );
            let angle =
                crate::scales::Scale::angle(meter.needles[channel].motion.position() as f32);
            let tip = Point::new(174.0 * angle.cos(), 174.0 * angle.sin());
            let midpoint = view.transform * (pivot + tip.to_vec2() * 0.5);
            let (x, y) = (midpoint.x as usize, midpoint.y as usize);
            assert!(
                (y - 1..=y + 1).any(
                    |y| (x - 1..=x + 1).any(|x| pixel(Point::new(x as f64, y as f64))[0] > 160)
                ),
                "each needle must render between its pivot and tip"
            );
        }
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255),
            "the meter frame is opaque"
        );
        if let Some(path) = std::env::var_os("VU_RENDER_PREVIEW_PATH") {
            let path = std::path::PathBuf::from(path);
            let path = match view.layout {
                Layout::Horizontal => path,
                Layout::Vertical => path.with_extension("vertical.png"),
            };
            let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&pixels)?;
        }
    }
    Ok(())
}

fn render_frame(
    device: &DeviceHandle,
    renderer: &mut Renderer,
    scene: &Scene,
    width: u32,
    height: u32,
) -> anyhow::Result<Vec<u8>> {
    let texture = device.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("VU rendering test"),
        size: wgpu::Extent3d {
            width,
            height,
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
        scene,
        &texture.create_view(&wgpu::TextureViewDescriptor::default()),
        &RenderParams {
            base_color: BACKGROUND,
            width,
            height,
            antialiasing_method: AaConfig::Msaa16,
        },
    )?;
    // Both test widths give rows aligned to wgpu's 256-byte copy requirement.
    let row_bytes = width * 4;
    let buffer = device.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VU rendering readback"),
        size: u64::from(row_bytes * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    device.queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.device.poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;
    let pixels = buffer.slice(..).get_mapped_range()?.to_vec();
    buffer.unmap();
    Ok(pixels)
}
