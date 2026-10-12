use std::{fs::File, io::BufWriter, sync::mpsc};

use vello::{
    kurbo::{Point, Rect, Size},
    util::{DeviceHandle, RenderContext},
    wgpu, RenderParams, Renderer, Scene,
};

use super::{Layout, Meter, View};
use crate::{
    config::PreampDb,
    frame_schedule::FrameRateLimit,
    render::{prepare_device, renderer, BackgroundCache, ANTIALIASING},
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
#[ignore = "requires a compute-capable GPU; measures 240 FPS rendering"]
fn profiles_moving_needles_at_240_fps() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    const FPS: u32 = 240;
    let budget = std::env::var_os("VU_PROFILE_MAX_RENDER_US")
        .map(|budget| -> anyhow::Result<f64> {
            let budget: f64 = budget
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("render budget must be UTF-8"))?
                .parse()?;
            anyhow::ensure!(
                budget.is_finite() && budget > 0.0,
                "render budget must be finite and positive"
            );
            Ok(budget)
        })
        .transpose()?;
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    let mut context = RenderContext::new();
    let id = pollster::block_on(prepare_device(
        &mut context,
        None,
        super::Renderer::device_preferences(),
    ))?;
    let device = &context.devices[id];
    let mut renderer = renderer(&device.device)?;
    let mut background = BackgroundCache::new();
    let mut scene = Scene::new();
    let view = View::fit(Size::new(1280.0, 440.0));
    let texture = device.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("VU profiling target"),
        size: wgpu::Extent3d {
            width: 1280,
            height: 440,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let params = RenderParams {
        base_color: crate::theme::Theme::new(crate::theme::ThemeId::default())
            .colors
            .background,
        width: 1280,
        height: 440,
        antialiasing_method: ANTIALIASING,
    };
    let output = device.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("VU profiling output"),
        ..texture_descriptor(1280, 440, wgpu::TextureUsages::RENDER_ATTACHMENT)
    });
    let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
    let mut overlay = super::Renderer::new(&device.device, output.format());
    let interval = Duration::from_secs_f64(1.0 / f64::from(FPS));
    let mut draw_time = Duration::ZERO;
    let mut render_time = Duration::ZERO;
    let mut rendered = 0;
    let mut start = Instant::now();
    for frame in 0..FPS * 6 {
        if frame == FPS {
            device.device.poll(wgpu::PollType::wait_indefinitely())?;
            start = Instant::now();
        }
        let levels = crate::profile::levels(f64::from(frame) / f64::from(FPS));
        meter.processor.consume_buf(&[levels; 200]);
        let before_draw = Instant::now();
        scene.reset();
        let frame_kind = meter.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps240,
            None,
            1.0 / FPS as f32,
        );
        let after_draw = Instant::now();
        let changed = !matches!(frame_kind.background, crate::render::Frame::Unchanged(_));
        if changed {
            let composite = background.compose(
                &mut renderer,
                device,
                frame_kind.background,
                &scene,
                &params,
            )?;
            renderer.render_to_texture(
                &device.device,
                &device.queue,
                composite,
                &target,
                &params,
            )?;
        }
        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        overlay.draw(
            device,
            &mut encoder,
            &target,
            &output_view,
            Size::new(1280.0, 440.0),
            &frame_kind.overlay,
        );
        device.queue.submit([encoder.finish()]);
        if frame >= FPS {
            draw_time += after_draw - before_draw;
            render_time += after_draw.elapsed();
            rendered += u32::from(changed);
            std::thread::sleep(
                (start + interval * (frame - FPS + 1)).saturating_duration_since(Instant::now()),
            );
        }
    }
    device.device.poll(wgpu::PollType::wait_indefinitely())?;
    let frames = FPS * 5;
    let render_us = render_time.as_secs_f64() * 1e6 / f64::from(frames);
    eprintln!(
        "240 FPS: {rendered}/{frames} base rasterizations; draw {:.1} us/frame; render {:.1} us/frame; elapsed {:.3}s",
        draw_time.as_secs_f64() * 1e6 / f64::from(frames),
        render_us,
        start.elapsed().as_secs_f64(),
    );
    if let Some(budget) = budget {
        assert!(
            render_us <= budget,
            "render time {render_us:.1} us exceeds {budget:.1} us/frame"
        );
    }
    Ok(())
}

#[test]
fn needle_motion_reuses_the_base_while_readout_fades_update_it() -> anyhow::Result<()> {
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.processor.set_samplerate(48000);
    for _ in 0..2 {
        meter.processor.consume_buf(&[[0.1, 0.5]; 14400]);
    }
    let view = View::fit(Size::new(1280.0, 440.0));
    let mut scene = Scene::new();
    let first = meter.draw(&mut scene, &view, FrameRateLimit::Fps240, None, 1.0 / 240.0);
    assert!(matches!(first.background, crate::render::Frame::Rebuilt(_)));
    let first_tip = first.overlay.needles[0].screen_tip();
    scene.reset();
    let moving = meter.draw(&mut scene, &view, FrameRateLimit::Fps240, None, 1.0 / 240.0);
    assert!(matches!(
        moving.background,
        crate::render::Frame::Unchanged(_)
    ));
    assert!(moving.overlay.needles[0].screen_tip().distance(first_tip) > 0.2);
    scene.reset();
    let shown = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        Some(crate::scales::ScaleSetting::DbRange),
        0.25,
    );
    assert!(matches!(
        shown.background,
        crate::render::Frame::Retained(_)
    ));
    scene.reset();
    let fading = meter.draw(&mut scene, &view, FrameRateLimit::Fps240, None, 1.0 / 240.0);
    assert!(matches!(
        fading.background,
        crate::render::Frame::Retained(_)
    ));
    meter.set_theme(
        crate::theme::ThemeId::default().adjusted(crate::frame_schedule::Adjustment::Increase),
    );
    for dt in [0.05, 0.05, 0.1] {
        scene.reset();
        let frame = meter.draw(&mut scene, &view, FrameRateLimit::Fps240, None, dt);
        assert!(matches!(frame.background, crate::render::Frame::Rebuilt(_)));
    }
    scene.reset();
    let settled = meter.draw(&mut scene, &view, FrameRateLimit::Fps240, None, 2.0);
    assert!(matches!(
        settled.background,
        crate::render::Frame::Retained(_)
    ));
    scene.reset();
    let cached = meter.draw(&mut scene, &view, FrameRateLimit::Fps240, None, 0.1);
    assert!(matches!(
        cached.background,
        crate::render::Frame::Unchanged(_)
    ));
    Ok(())
}

#[test]
#[ignore = "requires a compute-capable GPU"]
fn renders_stereo_scales_text_needles_and_overload() -> anyhow::Result<()> {
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
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
            base_color: crate::theme::Theme::new(crate::theme::ThemeId::default())
                .colors
                .background,
            width,
            height,
            antialiasing_method: ANTIALIASING,
        };
        let composite = background.compose(
            &mut renderer,
            device,
            crate::render::Frame::Retained(&meter.face),
            &scene,
            &params,
        )?;
        let pixels = render_frame(
            device,
            &mut renderer,
            composite,
            Some(&meter.appearance.as_ref().unwrap().needles),
            width,
            height,
        )?;
        let pixel = |point: Point| {
            let (x, y) = (point.x as usize, point.y as usize);
            &pixels[(y * width as usize + x) * 4..][..4]
        };
        let background_rgb = meter.theme.colors.background.components;
        let scale_contrast: f32 = meter.theme.colors.scale.components[..3]
            .iter()
            .zip(&background_rgb[..3])
            .map(|(ink, background)| (ink - background).abs() * 255.0)
            .sum();
        let count_scale_ink = |region: Rect| {
            let region = view.transform.transform_rect_bbox(region);
            (region.y0 as usize..region.y1 as usize)
                .flat_map(|y| {
                    (region.x0 as usize..region.x1 as usize)
                        .map(move |x| Point::new(x as f64, y as f64))
                })
                .filter(|&point| {
                    let p = pixel(point);
                    let contrast: f32 = p[..3]
                        .iter()
                        .zip(&background_rgb[..3])
                        .map(|(pixel, background)| (f32::from(*pixel) - background * 255.0).abs())
                        .sum();
                    contrast > scale_contrast * 0.5
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
                count_scale_ink(scale_region) > 400,
                "both scales must render in either layout"
            );
            let label_region =
                Rect::new(origin.x, origin.y + 7.5, origin.x + 320.0, origin.y + 27.5);
            assert!(
                count_scale_ink(label_region) > 50,
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
    needles: Option<&[super::NeedleAppearance; 2]>,
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
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    renderer.render_to_texture(
        &device.device,
        &device.queue,
        scene,
        &texture.create_view(&wgpu::TextureViewDescriptor::default()),
        &RenderParams {
            base_color: crate::theme::Theme::new(crate::theme::ThemeId::default())
                .colors
                .background,
            width,
            height,
            antialiasing_method: ANTIALIASING,
        },
    )?;
    // Test widths keep readback rows aligned to wgpu's 256-byte copy requirement.
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
    let output = needles.map(|needles| {
        let output = device.device.create_texture(&texture_descriptor(
            width,
            height,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        ));
        super::Renderer::new(&device.device, texture.format()).draw(
            device,
            &mut encoder,
            &texture.create_view(&wgpu::TextureViewDescriptor::default()),
            &output.create_view(&wgpu::TextureViewDescriptor::default()),
            Size::new(f64::from(width), f64::from(height)),
            &super::Overlay {
                needles,
                colors: crate::theme::Theme::new(crate::theme::ThemeId::default()).colors,
            },
        );
        output
    });
    let source = output.as_ref().unwrap_or(&texture);
    encoder.copy_texture_to_buffer(
        source.as_image_copy(),
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

fn texture_descriptor(
    width: u32,
    height: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    }
}

#[test]
#[ignore = "requires a compute-capable GPU"]
fn overlay_matches_vector_stroke_sweep_mask_and_overload() -> anyhow::Result<()> {
    for (binding, preferences) in [
        ("buffered", wgpu::DeviceDescriptor::default()),
        ("preferred", super::Renderer::device_preferences()),
    ] {
        let mut context = RenderContext::new();
        let id = pollster::block_on(prepare_device(&mut context, None, preferences))?;
        let device = &context.devices[id];
        let mut renderer = renderer(&device.device)?;
        for (name, width, height, dt, steps, zoom, overload, shutter_frames) in [
            (
                "bent-rise15",
                1280,
                440,
                1.0 / 15.0,
                1,
                1.0,
                [0.0, 0.0],
                1.0,
            ),
            ("bent-zoom", 1280, 440, 1.0 / 30.0, 2, 1.7, [0.0, 0.0], 1.0),
            (
                "bent-shutter2.5",
                640,
                960,
                1.0 / 60.0,
                4,
                1.0,
                [0.0, 0.0],
                2.5,
            ),
            ("bent", 1280, 440, 1.0 / 60.0, 90, 1.0, [0.0, 0.0], 1.0),
            ("rest", 1280, 440, 1.0 / 60.0, 90, 1.0, [0.0, 0.0], 1.0),
            ("rise240", 1280, 440, 1.0 / 240.0, 3, 1.0, [0.0, 0.0], 1.0),
            ("rise15", 1280, 440, 1.0 / 15.0, 1, 1.0, [0.0, 0.0], 1.0),
            ("vertical", 640, 960, 1.0 / 15.0, 2, 1.0, [0.0, 0.0], 1.0),
            ("zoom", 1280, 440, 1.0 / 30.0, 2, 1.7, [0.0, 0.0], 1.0),
            ("subpixel", 320, 220, 1.0 / 60.0, 90, 1.0, [0.0, 0.0], 1.0),
            ("overload", 1280, 440, 1.0 / 60.0, 90, 1.0, [0.45, 1.0], 1.0),
            ("shutter2", 1280, 440, 1.0 / 60.0, 4, 1.0, [0.0, 0.0], 2.0),
            ("shutter2.5", 640, 960, 1.0 / 60.0, 4, 1.0, [0.0, 0.0], 2.5),
            (
                "shutter16",
                1280,
                440,
                1.0 / 60.0,
                16,
                1.0,
                [0.0, 0.0],
                16.0,
            ),
        ] {
            let mut meter = Meter::new(&crate::config::Config {
                preamp_db: PreampDb::try_from(0.0)?,
                shutter_frames: crate::config::ShutterFrames::try_from(shutter_frames)?,
                ..Default::default()
            })?;
            meter.audio_event(AudioEvent::Config { samplerate: 48000 });
            for _ in 0..2 {
                meter.processor.consume_buf(&[[0.1, 1.0]; 14400]);
            }
            let mut view = View::fit(Size::new(f64::from(width), f64::from(height)));
            view.zoom(
                Point::new(f64::from(width) / 2.0, f64::from(height) / 2.0),
                zoom,
            );
            let mut scene = Scene::new();
            for _ in 0..steps {
                scene.reset();
                meter.draw(&mut scene, &view, FrameRateLimit::Fps240, None, dt);
            }
            let mut needles = meter.appearance.as_ref().unwrap().needles;
            for (needle, overload) in needles.iter_mut().zip(overload) {
                needle.overload = overload;
            }
            if name.starts_with("bent") {
                let mut motion = super::Motion::new();
                let exposure = motion.advance(1.2, 1.0, crate::config::ShutterFrames::default());
                for needle in &mut needles {
                    needle.bend = exposure.bend;
                    if name == "bent" {
                        needle.blur = 0.0;
                        needle.peak = 1.0;
                    }
                }
            }
            let mut reference = Scene::new();
            for needle in &needles {
                draw_vector_reference(&mut reference, needle);
            }
            let expected = render_frame(device, &mut renderer, &reference, None, width, height)?;
            let actual = render_frame(
                device,
                &mut renderer,
                &Scene::new(),
                Some(&needles),
                width,
                height,
            )?;
            let error: u64 = actual
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected.as_chunks::<4>().0)
                .flat_map(|(actual, expected)| actual[..3].iter().zip(&expected[..3]))
                .map(|(actual, expected)| u64::from(actual.abs_diff(*expected)))
                .sum();
            let ink: u64 = expected
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|pixel| {
                    pixel[..3].iter().zip(
                        meter.theme.colors.background.components[..3]
                            .iter()
                            .map(|value| (value * 255.0).round() as u8),
                    )
                })
                .map(|(pixel, background)| u64::from(pixel.abs_diff(background)))
                .sum();
            let relative_error = error as f64 / ink as f64;
            eprintln!("{binding}/{name}: overlay/vector RGB error {relative_error:.3}");
            if let Some(path) = std::env::var_os("VU_RENDER_PREVIEW_PATH") {
                let path = std::path::PathBuf::from(path);
                for (kind, pixels) in [("reference", &expected), ("overlay", &actual)] {
                    let mut encoder = png::Encoder::new(
                        BufWriter::new(File::create(
                            path.with_extension(format!("{binding}.{name}.{kind}.png")),
                        )?),
                        width,
                        height,
                    );
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.set_depth(png::BitDepth::Eight);
                    encoder.write_header()?.write_image_data(pixels)?;
                }
            }
            assert!(
                relative_error < 0.08,
                "{binding}/{name}: overlay differs from vector coverage, gradients or opacity mask"
            );
        }
    }
    Ok(())
}

fn draw_vector_reference(scene: &mut Scene, needle: &super::NeedleAppearance) {
    use super::{GLOW_RADIUS, LAMP_CENTER, LAMP_RADIUS, NEEDLE_RADIUS, NEEDLE_WIDTH};
    use vello::{
        kurbo::{Circle, Line, Stroke, Triangle},
        peniko::{Color, Compose, Fill, Gradient},
    };
    let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
    let ink = colors.needle;
    let transform = needle.transform;
    let strength = needle.overload;
    let light = LAMP_CENTER;
    if strength > 0.0 {
        let glow_color = crate::theme::mix(colors.warning, colors.lamp_hot, strength.powi(4) * 0.1);
        let stops: [(f32, Color); 17] = std::array::from_fn(|index| {
            let position = index as f32 / 16.0;
            (
                position,
                glow_color.with_alpha(strength.powi(2) * 0.2 * (1.0 - position).powi(3)),
            )
        });
        let glow =
            Gradient::new_two_point_radial(light, LAMP_RADIUS as f32, light, GLOW_RADIUS as f32)
                .with_stops(stops);
        scene.fill(
            Fill::NonZero,
            transform,
            &glow,
            None,
            &Circle::new(light, GLOW_RADIUS),
        );
        scene.fill(
            Fill::NonZero,
            transform,
            crate::theme::mix(colors.warning, colors.lamp_hot, strength.powi(2))
                .with_alpha(strength),
            None,
            &Circle::new(light, LAMP_RADIUS),
        );
    }
    let tip = needle.bent_tip(needle.angle);
    let radius = tip.distance(Point::ORIGIN);
    let clip = Circle::new(Point::ORIGIN, radius + NEEDLE_WIDTH);
    if needle.peak < 1.0 {
        scene.push_layer(Fill::NonZero, Compose::SrcOver, 1.0, transform, &clip);
    }
    if needle.bend.radians() > 0.0 {
        let hinge = super::NeedleAppearance::hinge(needle.angle);
        let trim = super::BEND_ROUNDING;
        let start = hinge - (hinge - Point::ORIGIN) * (trim / super::HINGE_RADIUS);
        let end = hinge + (tip - hinge) * (trim / (NEEDLE_RADIUS - super::HINGE_RADIUS));
        let mut path = vello::kurbo::BezPath::new();
        path.move_to(Point::ORIGIN);
        path.line_to(start);
        path.quad_to(hinge, end);
        path.line_to(tip);
        scene.stroke(&Stroke::new(NEEDLE_WIDTH), transform, ink, None, &path);
    } else {
        scene.stroke(
            &Stroke::new(NEEDLE_WIDTH),
            transform,
            ink,
            None,
            &Line::new(Point::ORIGIN, tip),
        );
    }
    let [a, b, ..] = transform.as_coeffs();
    let scale = a.hypot(b);
    for edge in needle.edges {
        if needle.blur > 0.0 && (edge - needle.angle).abs() * radius * scale > 0.5 {
            if needle.bend.radians() == 0.0 {
                let triangle =
                    Triangle::new(Point::ORIGIN, tip, super::NeedleAppearance::tip(edge));
                let gradient = Gradient::new_sweep(Point::ORIGIN, needle.angle as f32, edge as f32)
                    .with_stops([ink.with_alpha(needle.blur), ink.with_alpha(0.0)]);
                scene.fill(Fill::NonZero, transform, &gradient, None, &triangle);
            } else {
                use vello::kurbo::{BezPath, ParamCurve, QuadBez, Shape};
                let hinge = super::NeedleAppearance::hinge(0.0);
                let tip = needle.bent_tip(0.0);
                let start =
                    hinge - (hinge - Point::ORIGIN) * (super::BEND_ROUNDING / super::HINGE_RADIUS);
                let end = hinge
                    + (tip - hinge)
                        * (super::BEND_ROUNDING / (NEEDLE_RADIUS - super::HINGE_RADIUS));
                let curve = QuadBez::new(start, hinge, end);
                let outer = Line::new(end, tip);
                // Vector annuli approximate the spatial warp, with a sweep gradient in each band.
                for band in 0..=48 {
                    let low_radius = if band == 0 {
                        0.0
                    } else {
                        start.x + (radius - start.x) * (band - 1) as f64 / 48.0
                    };
                    let high_radius = if band == 0 {
                        start.x
                    } else {
                        start.x + (radius - start.x) * band as f64 / 48.0
                    };
                    let mut low = 0.0;
                    let mut high = 1.0;
                    let target_radius = (low_radius + high_radius) / 2.0;
                    for _ in 0..40 {
                        let fraction = (low + high) / 2.0;
                        let point = if fraction < 0.5 {
                            curve.eval(fraction * 2.0)
                        } else {
                            outer.eval(fraction * 2.0 - 1.0)
                        };
                        if point.distance(Point::ORIGIN) < target_radius {
                            low = fraction;
                        } else {
                            high = fraction;
                        }
                    }
                    let fraction = (low + high) / 2.0;
                    let point = if fraction < 0.5 {
                        curve.eval(fraction * 2.0)
                    } else {
                        outer.eval(fraction * 2.0 - 1.0)
                    };
                    let phase = point.y.atan2(point.x);
                    let at = |angle: f64| Point::new(radius * angle.cos(), radius * angle.sin());
                    let triangle =
                        Triangle::new(Point::ORIGIN, at(needle.angle + phase), at(edge + phase));
                    let gradient = Gradient::new_sweep(
                        Point::ORIGIN,
                        (needle.angle + phase) as f32,
                        (edge + phase) as f32,
                    )
                    .with_stops([ink.with_alpha(needle.blur), ink.with_alpha(0.0)]);
                    let mut ring = BezPath::new();
                    ring.extend(Circle::new(Point::ORIGIN, high_radius).path_elements(0.01));
                    if low_radius > 0.0 {
                        ring.extend(Circle::new(Point::ORIGIN, low_radius).path_elements(0.01));
                    }
                    scene.push_layer(Fill::EvenOdd, Compose::SrcOver, 1.0, transform, &ring);
                    scene.fill(Fill::NonZero, transform, &gradient, None, &triangle);
                    scene.pop_layer();
                }
            }
        }
    }
    if needle.peak < 1.0 {
        let core = needle.peak.sqrt();
        let white = Color::from_rgb8(255, 255, 255);
        let opacity = |coverage| white.with_alpha(1.0 - needle.blur + needle.blur * coverage);
        let mask = Gradient::new_radial(Point::ORIGIN, radius as f32).with_stops([
            (0.0, white),
            (needle.peak, white),
            (core, opacity(core)),
            (1.0, opacity(needle.peak)),
        ]);
        scene.push_layer(Fill::NonZero, Compose::DestIn, 1.0, transform, &clip);
        scene.fill(Fill::NonZero, transform, &mask, None, &clip);
        scene.pop_layer();
        scene.pop_layer();
    }
}
