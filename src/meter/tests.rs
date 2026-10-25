use std::{
    fs::File,
    io::BufWriter,
    sync::mpsc,
    time::{Duration, Instant},
};

use vello::{
    kurbo::{Point, Rect, Size},
    util::{DeviceHandle, RenderContext},
    wgpu, RenderParams, Renderer, Scene,
};

use super::{Layout, Meter, View};
use crate::{
    config::PreampDb,
    frame_schedule::{FrameRateLimit, FrameTime},
    render::{prepare_device, renderer, BackgroundCache, ANTIALIASING},
    AudioEvent,
};

fn gauges<'a>(overlay: &'a super::Overlay<'a>) -> &'a super::GaugeOverlay<'a> {
    match overlay {
        super::Overlay::Gauges(gauge) => gauge,
        super::Overlay::Visualizer(_) => panic!("the gauge renderer needs a gauge overlay"),
    }
}

/// Captured sample time and redraw time can advance independently in tests.
struct TestClock {
    origin: Instant,
    samples: u64,
    display: Instant,
}

impl TestClock {
    fn new() -> Self {
        let origin = Instant::now();
        Self {
            origin,
            samples: 0,
            display: origin,
        }
    }

    fn feed(&mut self, meter: &mut Meter, buf: &[[f32; 2]]) {
        meter.audio_event(AudioEvent::Buffer {
            buf: buf.to_vec(),
            captured_at: self.origin + Duration::from_secs_f64(self.samples as f64 / 48000.0),
        });
        self.samples += buf.len() as u64;
    }

    fn frame(&mut self, seconds: f64) -> FrameTime {
        let start = self.display;
        self.display += Duration::from_secs_f64(seconds);
        FrameTime::new(start, self.display)
    }
}

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
    let mut clock = TestClock::new();
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
        clock.feed(&mut meter, &[levels; 200]);
        let before_draw = Instant::now();
        scene.reset();
        let frame_kind = meter.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps240,
            None,
            clock.frame(f64::from(1.0 / FPS as f32)),
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
            gauges(&frame_kind.overlay),
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
    let mut clock = TestClock::new();
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        mode: crate::measurement::Mode::Rms,
        ..Default::default()
    })?;
    meter.processor.set_samplerate(48000);
    for _ in 0..2 {
        clock.feed(&mut meter, &[[0.5, 1.0]; 14400]);
    }
    clock.display = clock.origin + Duration::from_millis(50);
    let view = View::fit(Size::new(1280.0, 440.0));
    let mut scene = Scene::new();
    let first = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        None,
        clock.frame(1.0 / 240.0),
    );
    assert!(matches!(first.background, crate::render::Frame::Rebuilt(_)));
    let first_tip = gauges(&first.overlay).needles[0].screen_tip();
    scene.reset();
    let moving = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        None,
        clock.frame(1.0 / 240.0),
    );
    assert!(matches!(
        moving.background,
        crate::render::Frame::Unchanged(_)
    ));
    assert!(
        gauges(&moving.overlay).needles[0]
            .screen_tip()
            .distance(first_tip)
            > 0.2
    );
    scene.reset();
    let shown = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        Some(crate::scales::ScaleSetting::DbRange),
        clock.frame(0.25),
    );
    assert!(matches!(
        shown.background,
        crate::render::Frame::Retained(_)
    ));
    scene.reset();
    let fading = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        None,
        clock.frame(1.0 / 240.0),
    );
    assert!(matches!(
        fading.background,
        crate::render::Frame::Retained(_)
    ));
    meter.set_theme(
        crate::theme::ThemeId::default().adjusted(crate::frame_schedule::Adjustment::Increase),
    );
    for dt in [0.05, 0.05, 0.1] {
        scene.reset();
        let frame = meter.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps240,
            None,
            clock.frame(dt),
        );
        assert!(matches!(frame.background, crate::render::Frame::Rebuilt(_)));
    }
    scene.reset();
    let settled = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        None,
        clock.frame(2.0),
    );
    assert!(matches!(
        settled.background,
        crate::render::Frame::Retained(_)
    ));
    scene.reset();
    let cached = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        None,
        clock.frame(0.1),
    );
    assert!(matches!(
        cached.background,
        crate::render::Frame::Unchanged(_)
    ));
    Ok(())
}

#[test]
#[ignore = "requires a compute-capable GPU"]
fn renders_all_measurement_modes_and_units_in_both_layouts() -> anyhow::Result<()> {
    let mut clock = TestClock::new();
    use crate::{measurement::Mode, meter::PeakMovement};
    let mut context = RenderContext::new();
    let id = pollster::block_on(context.device(None)).expect("a compute-capable GPU is required");
    let device = &context.devices[id];
    let mut renderer = renderer(&device.device)?;
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    let mut background = BackgroundCache::new();
    let mut scene = Scene::new();
    for mode in [Mode::Vu, Mode::Ppm, Mode::Rms, Mode::SamplePeak] {
        meter.set_mode(mode);
        let movements: &[PeakMovement] = if mode.uses_peak_movement() {
            &[PeakMovement::FullSpec, PeakMovement::VintageFr]
        } else {
            &[PeakMovement::FullSpec]
        };
        for &movement in movements {
            if mode.uses_peak_movement() {
                meter.set_peak_movement(movement);
            }
            for (width, height, layout_name) in [(1280, 440, "horizontal"), (640, 960, "vertical")]
            {
                let view = View::fit(Size::new(f64::from(width), f64::from(height)));
                for frame in 0..60 {
                    let buf: Vec<_> = (frame * 800..(frame + 1) * 800)
                        .map(|sample| {
                            let wave =
                                (std::f32::consts::TAU * 1000.0 * sample as f32 / 48000.0).sin();
                            [wave * 0.25, wave * 0.75]
                        })
                        .collect();
                    clock.feed(&mut meter, &buf);
                    scene.reset();
                    meter.draw(
                        &mut scene,
                        &view,
                        FrameRateLimit::Fps60,
                        None,
                        clock.frame(1.0 / 60.0),
                    );
                }
                let params = RenderParams {
                    base_color: meter.theme.colors.background,
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
                let background_rgb = meter.theme.colors.background.components;
                let ink = |region: Rect| {
                    (region.y0.max(0.0) as usize..region.y1.min(f64::from(height)) as usize)
                        .flat_map(|y| {
                            (region.x0.max(0.0) as usize..region.x1.min(f64::from(width)) as usize)
                                .map(move |x| (x, y))
                        })
                        .filter(|&(x, y)| {
                            pixels[(y * width as usize + x) * 4..][..3]
                                .iter()
                                .zip(&background_rgb[..3])
                                .any(|(&pixel, &background)| {
                                    (f32::from(pixel) - background * 255.0).abs() > 20.0
                                })
                        })
                        .count()
                };
                let [a, b, ..] = view.transform.as_coeffs();
                let scale = a.hypot(b);
                let baseline = f64::from(height) - 24.0 - 24.0 * scale;
                assert!(
                    ink(Rect::new(
                        24.0,
                        baseline - 13.0 * scale,
                        24.0 + meter.mode_readout.inline_width(&meter.mode_value) * scale,
                        baseline
                    )) > 40,
                    "mode control must render for {mode:?}/{layout_name}"
                );
                if mode.uses_peak_movement() {
                    let x = 24.0;
                    let baseline = baseline - 24.0 * scale;
                    assert!(
                        ink(Rect::new(
                            x,
                            baseline - 13.0 * scale,
                            x + meter
                                .peak_movement_readout
                                .inline_width(&meter.peak_movement_value)
                                * scale,
                            baseline
                        )) > 40,
                        "movement control must render for {mode:?}/{movement:?}/{layout_name}"
                    );
                }
                for pivot in view.layout.pivots() {
                    let center = view.transform
                        * (pivot + (super::LAMP_CENTER.x, super::LAMP_CENTER.y - 23.0));
                    let half_width = meter.units.width() * scale / 2.0;
                    assert!(
                        ink(Rect::new(
                            center.x - half_width,
                            center.y - 14.0 * scale,
                            center.x + half_width,
                            center.y
                        )) > 30,
                        "units must render for {mode:?}/{layout_name}"
                    );
                }
                if let Some(prefix) = std::env::var_os("VU_MODE_PREVIEW_PREFIX") {
                    let path = format!(
                        "{}-{mode:?}-{movement:?}-{layout_name}.png",
                        std::path::Path::new(&prefix).display()
                    );
                    let mut encoder =
                        png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.set_depth(png::BitDepth::Eight);
                    encoder.write_header()?.write_image_data(&pixels)?;
                }
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires a compute-capable GPU"]
fn renders_stereo_scales_text_needles_and_overload() -> anyhow::Result<()> {
    let mut clock = TestClock::new();
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    // Fill the RMS window, then deliver a buffer whose first frame has a full window.
    for _ in 0..2 {
        clock.feed(&mut meter, &[[0.1, 1.0]; 14400]);
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
            meter.draw(&mut scene, &view, limit, None, clock.frame(1.0 / 60.0));
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

#[test]
fn ui_scale_is_independent_of_window_size_and_zoom() {
    let mut view = View::fit(Size::new(1280.0, 440.0));
    assert_eq!(view.ui_scale(), 1.0);
    view.set_ui_scale(2.0);
    assert_eq!(view.ui_scale(), 2.0);
    view.zoom(Point::new(640.0, 220.0), 1.5);
    assert_eq!(view.ui_scale(), 2.0);
    view = View::fit(Size::new(640.0, 960.0));
    assert_eq!(view.ui_scale(), 1.0);
}

#[test]
fn text_scale_changes_the_visualizer_chrome_size() -> anyhow::Result<()> {
    let mut meter = Meter::new(&crate::config::Config {
        presentation: crate::visualizer::Presentation::Visualizer,
        ..Default::default()
    })?;
    let smaller = meter
        .text_scale
        .adjusted(crate::frame_schedule::Adjustment::Decrease);
    meter.set_text_scale(smaller);
    assert_eq!(meter.text_scale, smaller);
    assert!(meter.face_settings.is_none());
    assert!(meter.text_scale.percent() < 100);
    Ok(())
}

#[test]
fn debug_refresh_cycles_from_fft_hop_to_slow_rates() {
    use super::DebugRefresh;
    let mut refresh = DebugRefresh::default();
    assert_eq!(refresh, DebugRefresh::Hz10);
    assert_eq!(refresh.interval(), Some(0.1));
    refresh = refresh.next();
    assert_eq!(refresh, DebugRefresh::Hz5);
    refresh = refresh.next();
    assert_eq!(refresh, DebugRefresh::Hz2);
    refresh = refresh.next();
    assert_eq!(refresh, DebugRefresh::Hz1);
    refresh = refresh.next();
    assert_eq!(refresh, DebugRefresh::PerHop);
    assert_eq!(refresh.interval(), None);
    refresh = refresh.next();
    assert_eq!(refresh, DebugRefresh::Hz20);
    refresh = refresh.next();
    assert_eq!(refresh, DebugRefresh::Hz10);
}

#[test]
fn switching_presentation_swaps_overlays_and_keeps_audio_running() -> anyhow::Result<()> {
    use crate::visualizer::Presentation;
    let mut clock = TestClock::new();
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    for _ in 0..8 {
        clock.feed(&mut meter, &[[0.4, 0.8]; 800]);
    }
    clock.display = clock.origin + Duration::from_millis(120);
    let view = View::fit(Size::new(1280.0, 440.0));
    let mut scene = Scene::new();
    let gauge_angle = {
        let gauge = meter.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps60,
            None,
            clock.frame(1.0 / 60.0),
        );
        gauges(&gauge.overlay).needles[0].angle
    };
    meter.set_presentation(Presentation::Visualizer);
    assert_eq!(meter.presentation(), Presentation::Visualizer);
    scene.reset();
    {
        let visualizer = meter.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps60,
            None,
            clock.frame(1.0 / 60.0),
        );
        match &visualizer.overlay {
            super::Overlay::Visualizer(frame) => {
                assert!(frame.lines.len() + frame.ribbons.len() >= 12);
            }
            super::Overlay::Gauges(_) => panic!("the wireframe presentation must supply lines"),
        }
    }
    meter.set_presentation(Presentation::Gauges);
    scene.reset();
    let gauge = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps60,
        None,
        clock.frame(1.0 / 60.0),
    );
    let returned = gauges(&gauge.overlay);
    assert!((returned.needles[0].angle - gauge_angle).abs() > 1e-6);
    Ok(())
}

#[test]
#[ignore = "requires a compute-capable GPU"]
fn renders_visualizer_face_and_wireframe_with_hud() -> anyhow::Result<()> {
    use crate::visualizer::Presentation;
    let width = 1280;
    let height = 440;
    let mut clock = TestClock::new();
    let mut meter = Meter::new(&crate::config::Config {
        presentation: Presentation::Visualizer,
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    meter.set_debug(true);
    let view = View::fit(Size::new(f64::from(width), f64::from(height)));
    let mut scene = Scene::new();
    for frame in 0..240 {
        let buf: Vec<_> = (0..800)
            .map(|index| {
                let sample = frame * 800 + index;
                let time = sample as f32 / 48000.0;
                let block = sample % 9600;
                let kick = if block < 3_000 {
                    (1.0 - block as f32 / 3_000.0) * 0.9
                } else {
                    0.0
                };
                let tone = 0.22 * (std::f32::consts::TAU * 880.0 * time).sin()
                    + 0.12 * (std::f32::consts::TAU * 220.0 * time).sin();
                let low = kick * (std::f32::consts::TAU * 55.0 * time).sin();
                [tone + low; 2]
            })
            .collect();
        clock.feed(&mut meter, &buf);
        scene.reset();
        meter.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps60,
            None,
            clock.frame(1.0 / 60.0),
        );
    }
    let (lines, ribbons): (Vec<_>, Vec<_>) = {
        scene.reset();
        let frame = meter.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps60,
            None,
            clock.frame(1.0 / 60.0),
        );
        match &frame.overlay {
            super::Overlay::Visualizer(frame) => (frame.lines.to_vec(), frame.ribbons.to_vec()),
            super::Overlay::Gauges(_) => panic!("the visualizer must supply line overlay"),
        }
    };
    assert!(lines.len() + ribbons.len() >= 12);
    let mut context = RenderContext::new();
    let id = pollster::block_on(context.device(None)).expect("a compute-capable GPU is required");
    let device = &context.devices[id];
    let mut renderer = renderer(&device.device)?;
    let params = RenderParams {
        base_color: meter.theme.colors.background,
        width,
        height,
        antialiasing_method: ANTIALIASING,
    };
    let background_texture = device.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Visualizer face"),
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
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut background = BackgroundCache::new();
    let composite = background.compose(
        &mut renderer,
        device,
        crate::render::Frame::Rebuilt(&meter.face),
        &scene,
        &params,
    )?;
    renderer.render_to_texture(
        &device.device,
        &device.queue,
        composite,
        &background_texture.create_view(&wgpu::TextureViewDescriptor::default()),
        &params,
    )?;
    let output = device.device.create_texture(&texture_descriptor(
        width,
        height,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    ));
    let mut visualizer = crate::visualizer::Renderer::new(&device.device, output.format());
    let mut encoder = device
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    visualizer.draw(
        device,
        &mut encoder,
        &background_texture.create_view(&wgpu::TextureViewDescriptor::default()),
        &output.create_view(&wgpu::TextureViewDescriptor::default()),
        Size::new(f64::from(width), f64::from(height)),
        crate::visualizer::Frame {
            lines: &lines,
            ribbons: &ribbons,
        },
    );
    device.queue.submit([encoder.finish()]);
    let pixels = read_texture(device, &output, width, height)?;
    let background_rgb = meter.theme.colors.background.components;
    let ink = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| {
            pixel[..3]
                .iter()
                .zip(&background_rgb[..3])
                .any(|(pixel, background)| (f32::from(*pixel) - background * 255.0).abs() > 20.0)
        })
        .count();
    eprintln!("visualizer meter ink pixels: {ink}");
    assert!(ink > 3_000);
    if let Some(path) = std::env::var_os("VU_RENDER_PREVIEW_PATH") {
        let path = std::path::PathBuf::from(path).with_extension("visualizer-meter.png");
        let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
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
            &super::GaugeOverlay {
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

fn read_texture(
    device: &DeviceHandle,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> anyhow::Result<Vec<u8>> {
    let row_bytes = width * 4;
    let buffer = device.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VU texture readback"),
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
    let mut clock = TestClock::new();
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
                clock.feed(&mut meter, &[[0.1, 1.0]; 14400]);
            }
            let mut view = View::fit(Size::new(f64::from(width), f64::from(height)));
            view.zoom(
                Point::new(f64::from(width) / 2.0, f64::from(height) / 2.0),
                zoom,
            );
            let mut scene = Scene::new();
            for _ in 0..steps {
                scene.reset();
                meter.draw(
                    &mut scene,
                    &view,
                    FrameRateLimit::Fps240,
                    None,
                    clock.frame(dt),
                );
            }
            let mut needles = meter.appearance.as_ref().unwrap().needles;
            for (needle, overload) in needles.iter_mut().zip(overload) {
                needle.overload = overload;
            }
            if name.starts_with("bent") {
                let mut motion = super::Motion::new(
                    crate::measurement::Mode::Rms,
                    super::PeakMovement::default(),
                    crate::audio::timeline::AudioTime::ZERO,
                );
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

#[test]
fn startup_and_delayed_delivery_do_not_change_confirmed_measurements() -> anyhow::Result<()> {
    use crate::measurement::Mode;
    for mode in [Mode::Vu, Mode::Ppm, Mode::Rms, Mode::SamplePeak] {
        for rate in [44100, 48000, 96000] {
            let config = crate::config::Config {
                mode,
                preamp_db: PreampDb::try_from(0.0)?,
                ..Default::default()
            };
            let input: Vec<_> = (0..rate / 2)
                .map(|n| {
                    let tone = (n as f32 * 0.25).sin();
                    let amplitude = if n > rate / 10 && n < rate / 5 {
                        1.2
                    } else {
                        0.1
                    };
                    [tone * amplitude, -tone * amplitude * 0.5]
                })
                .collect();
            let origin = Instant::now();
            let mut reference = Meter::new(&config)?;
            reference.audio_event(AudioEvent::Config { samplerate: rate });
            reference.audio_event(AudioEvent::Buffer {
                buf: input.clone(),
                captured_at: origin,
            });
            for (packet, fps, startup, delivery_lag) in [
                (137, 240, 0.0, 0.0),
                (512, 60, 0.2, 0.0),
                (1024, 15, 0.2, 0.025),
            ] {
                let mut meter = Meter::new(&config)?;
                meter.audio_event(AudioEvent::Config { samplerate: rate });
                let mut samples = 0;
                let mut clock = TestClock {
                    origin,
                    samples: 0,
                    display: origin + Duration::from_secs_f64(startup),
                };
                let mut scene = Scene::new();
                let view = View::fit(Size::new(1280.0, 440.0));
                let dt = Duration::from_secs_f64(1.0 / fps as f64);
                for chunk in input.chunks(packet) {
                    let captured_at =
                        origin + Duration::from_secs_f64(samples as f64 / rate as f64);
                    let delivered_at = captured_at
                        + Duration::from_secs_f64(chunk.len() as f64 / rate as f64 + delivery_lag);
                    while clock.display + dt < delivered_at {
                        scene.reset();
                        meter.draw(
                            &mut scene,
                            &view,
                            FrameRateLimit::Fps240,
                            None,
                            clock.frame(dt.as_secs_f64()),
                        );
                    }
                    meter.audio_event(AudioEvent::Buffer {
                        buf: chunk.to_vec(),
                        captured_at,
                    });
                    samples += chunk.len();
                }
                assert_eq!(
                    reference.processor.audio_time(),
                    meter.processor.audio_time()
                );
                assert_eq!(
                    reference.processor.latest_levels(),
                    meter.processor.latest_levels()
                );
                for (expected, actual) in reference.needles.iter().zip(&meter.needles) {
                    assert_eq!(
                        expected.motion.confirmed_state(),
                        actual.motion.confirmed_state(),
                        "{mode:?}, {rate}, {packet}/{fps}"
                    );
                    assert_eq!(expected.overload.strength, actual.overload.strength);
                }
            }
        }
    }
    Ok(())
}

#[test]
fn first_shutter_excludes_startup_history_and_mode_changes_keep_sample_time() -> anyhow::Result<()>
{
    let mut meter = Meter::new(&crate::config::Config {
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    let mut clock = TestClock::new();
    clock.feed(&mut meter, &[[0.8, 0.2]; 48000]);
    clock.display = clock.origin + Duration::from_secs_f64(1.0 - 1.0 / 240.0);
    let mut scene = Scene::new();
    let view = View::fit(Size::new(1280.0, 440.0));
    let first = meter.draw(
        &mut scene,
        &view,
        FrameRateLimit::Fps240,
        None,
        clock.frame(1.0 / 240.0),
    );
    assert!(
        (gauges(&first.overlay).needles[0].edges[1] - gauges(&first.overlay).needles[0].edges[0])
            .abs()
            < 0.001,
        "first exposure must cover the settled current needle, not its startup sweep"
    );
    let before = meter.processor.audio_time();
    meter.set_mode(crate::measurement::Mode::SamplePeak);
    assert_eq!(before, meter.processor.audio_time());
    clock.feed(&mut meter, &[[0.25, 0.5]; 480]);
    assert!(meter.needles[0].motion.confirmed_state().0 > before);
    Ok(())
}

#[test]
#[ignore = "measures CPU cost of audio ingestion and 240 FPS drawing without a GPU"]
fn profiles_audio_and_drawing() -> anyhow::Result<()> {
    for delivery_lag_ms in [0, 25] {
        for mode in [
            crate::measurement::Mode::Vu,
            crate::measurement::Mode::Rms,
            crate::measurement::Mode::Ppm,
            crate::measurement::Mode::SamplePeak,
        ] {
            let mut meter = Meter::new(&crate::config::Config {
                mode,
                preamp_db: PreampDb::try_from(0.0)?,
                ..Default::default()
            })?;
            meter.audio_event(AudioEvent::Config { samplerate: 48000 });
            let mut clock = TestClock::new();
            clock.display += Duration::from_millis(delivery_lag_ms);
            let mut scene = Scene::new();
            let view = View::fit(Size::new(1280.0, 440.0));
            let mut ingestion = Duration::ZERO;
            let mut drawing = Duration::ZERO;
            for frame in 0..48000 {
                let samples = [crate::profile::levels(frame as f64 / 240.0); 200];
                let before = Instant::now();
                clock.feed(&mut meter, &samples);
                ingestion += before.elapsed();
                scene.reset();
                let before = Instant::now();
                let result = meter.draw(
                    &mut scene,
                    &view,
                    FrameRateLimit::Fps240,
                    None,
                    clock.frame(1.0 / 240.0),
                );
                std::hint::black_box(result);
                drawing += before.elapsed();
            }
            eprintln!(
            "{mode:?}, {delivery_lag_ms} ms delivery lag, 240 FPS: ingest {:.2} us/frame; draw {:.2} us/frame; total {:.2} us/frame",
            ingestion.as_secs_f64() * 1e6 / 48000.0,
            drawing.as_secs_f64() * 1e6 / 48000.0,
            (ingestion + drawing).as_secs_f64() * 1e6 / 48000.0
        );
        }
    }
    Ok(())
}

#[test]
fn steady_vu_tone_stays_smooth_across_capture_buffer_arrivals() -> anyhow::Result<()> {
    use crate::measurement::Mode;
    let config = crate::config::Config {
        mode: Mode::Vu,
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    };
    let origin = Instant::now();
    let input: Vec<_> = (0..144000)
        .map(|sample| {
            let level =
                0.8 * (std::f64::consts::TAU * 100.0 * sample as f64 / 48000.0).sin() as f32;
            [level; 2]
        })
        .collect();
    let view = View::fit(Size::new(1280.0, 440.0));
    for packet in [512, 1024] {
        let mut reference = Meter::new(&config)?;
        let mut streamed = Meter::new(&config)?;
        for meter in [&mut reference, &mut streamed] {
            meter.audio_event(AudioEvent::Config { samplerate: 48000 });
        }
        let mut delivered = 0;
        let mut scene = Scene::new();
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for frame in 1..=720 {
            let samples = frame * 200;
            reference.audio_event(AudioEvent::Buffer {
                buf: input[samples - 200..samples].to_vec(),
                captured_at: origin + Duration::from_secs_f64((samples - 200) as f64 / 48000.0),
            });
            while delivered + packet <= samples {
                streamed.audio_event(AudioEvent::Buffer {
                    buf: input[delivered..delivered + packet].to_vec(),
                    captured_at: origin + Duration::from_secs_f64(delivered as f64 / 48000.0),
                });
                delivered += packet;
            }
            let time = FrameTime::new(
                origin + Duration::from_secs_f64((samples - 200) as f64 / 48000.0),
                origin + Duration::from_secs_f64(samples as f64 / 48000.0),
            );
            scene.reset();
            let expected = gauges(
                &reference
                    .draw(&mut scene, &view, FrameRateLimit::Fps240, None, time)
                    .overlay,
            )
            .needles[0]
                .angle;
            scene.reset();
            let actual = gauges(
                &streamed
                    .draw(&mut scene, &view, FrameRateLimit::Fps240, None, time)
                    .overlay,
            )
            .needles[0]
                .angle;
            if frame > 480 {
                let error_pixels = (actual - expected) * super::NEEDLE_RADIUS;
                minimum = minimum.min(error_pixels);
                maximum = maximum.max(error_pixels);
            }
        }
        eprintln!(
            "{packet}-sample buffers: display ripple {:.3} pixels",
            maximum - minimum
        );
        assert!(
            maximum - minimum < 0.1,
            "steady tone has visible buffer-correlated needle jitter"
        );
    }
    Ok(())
}

#[test]
fn playback_lookahead_is_sampled_at_frame_time_for_needles_and_lamps() -> anyhow::Result<()> {
    use crate::{config::ShutterFrames, measurement::Mode};
    let shutter = ShutterFrames::try_from(1.0)?;
    let mut meter = Meter::new(&crate::config::Config {
        mode: Mode::Vu,
        shutter_frames: shutter,
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    })?;
    meter.audio_event(AudioEvent::Config { samplerate: 48000 });
    let origin = Instant::now();
    let starts_at = origin + Duration::from_millis(12);
    // The entire block is already computed, but its sample timeline is ahead
    // of delivery. The displayed frame falls inside that known block.
    meter.audio_event(AudioEvent::Buffer {
        buf: vec![[1.5; 2]; 512],
        captured_at: starts_at,
    });
    let elapsed = 224.0 / 48000.0;
    let time = FrameTime::new(starts_at, starts_at + Duration::from_secs_f64(elapsed));
    let view = View::fit(Size::new(1280.0, 440.0));
    let mut scene = Scene::new();
    let needle = gauges(
        &meter
            .draw(&mut scene, &view, FrameRateLimit::Fps240, None, time)
            .overlay,
    )
    .needles[0];
    let level = (1.5 * std::f64::consts::PI / (2.0 * 2.0_f64.sqrt())) as f32;
    let target = Mode::Vu.position(&meter.scale, level);
    let mut motion = super::Motion::new(
        Mode::Vu,
        super::PeakMovement::default(),
        crate::audio::timeline::AudioTime::ZERO,
    );
    let expected = motion.advance_path(std::iter::once((target, elapsed)), shutter);
    let angle = crate::scales::Scale::angle(expected.average as f32) + std::f64::consts::TAU;
    assert!((needle.angle - angle).abs() < 1e-7);
    assert!(
        (needle.overload - (25.0 * elapsed) as f32).abs() < 1e-6,
        "the lamp must use the displayed sample time, not the end of the queued block"
    );
    Ok(())
}

#[test]
fn changing_vu_levels_keep_continuous_motion_and_full_shutter_between_buffers() -> anyhow::Result<()>
{
    use crate::measurement::Mode;
    let config = crate::config::Config {
        mode: Mode::Vu,
        preamp_db: PreampDb::try_from(0.0)?,
        ..Default::default()
    };
    let origin = Instant::now();
    let input: Vec<_> = (0..144000)
        .map(|sample| {
            let time = sample as f64 / 48000.0;
            let amplitude = 0.2 + 0.3 * time;
            let level = (amplitude * (std::f64::consts::TAU * 1000.0 * time).sin()) as f32;
            [level; 2]
        })
        .collect();
    let view = View::fit(Size::new(1280.0, 440.0));
    for packet in [512, 1024] {
        let mut meter = Meter::new(&config)?;
        meter.audio_event(AudioEvent::Config { samplerate: 48000 });
        let mut scene = Scene::new();
        let mut delivered = 0;
        let mut previous = None;
        let mut repeated = 0;
        let mut collapsed = 0;
        for frame in 1..=720 {
            let samples = frame * 200;
            while delivered + packet <= samples {
                meter.audio_event(AudioEvent::Buffer {
                    buf: input[delivered..delivered + packet].to_vec(),
                    captured_at: origin + Duration::from_secs_f64(delivered as f64 / 48000.0),
                });
                delivered += packet;
            }
            let time = FrameTime::new(
                origin + Duration::from_secs_f64((samples - 200) as f64 / 48000.0),
                origin + Duration::from_secs_f64(samples as f64 / 48000.0),
            );
            scene.reset();
            let needle = gauges(
                &meter
                    .draw(&mut scene, &view, FrameRateLimit::Fps240, None, time)
                    .overlay,
            )
            .needles[0];
            if frame > 240 {
                if previous == Some(needle.angle) {
                    repeated += 1;
                }
                if needle.edges[0] == needle.edges[1] {
                    collapsed += 1;
                }
            }
            previous = Some(needle.angle);
        }
        eprintln!("{packet}: {repeated} frozen frames, {collapsed} collapsed exposures out of 480");
        assert_eq!(
            repeated, 0,
            "moving needles must not wait for the next capture callback"
        );
        assert_eq!(
            collapsed, 0,
            "active motion must retain the full shutter path between callbacks"
        );
    }
    Ok(())
}

#[test]
fn embedded_gauge_style_preserves_measurements_and_separates_visual_roles() {
    use super::GaugeStyle;
    use crate::theme::{Accent, ThemeId};
    let config = crate::config::Config::default();
    let mut original = Meter::new(&config).unwrap();
    let mut styled = Meter::new(&config).unwrap();
    let mut colors = crate::theme::Theme::new(config.theme).colors;
    colors.needle = ThemeId::default().color(Accent::Cyan);
    colors.lamp = ThemeId::default().color(Accent::Yellow);
    colors.lamp_glow = ThemeId::default().color(Accent::Magenta);
    styled.set_gauge_style(GaugeStyle {
        line_width: Some(3.0),
        colors: Some(colors),
    });
    let mut clock = TestClock::new();
    for meter in [&mut original, &mut styled] {
        clock.samples = 0;
        clock.feed(meter, &[[0.5, 0.25]; 4800]);
    }
    let frame = FrameTime::new(clock.origin, clock.origin + Duration::from_millis(100));
    let transforms = [vello::kurbo::Affine::IDENTITY; 2];
    let original = original.update_gauges(transforms, frame);
    let styled = styled.update_gauges(transforms, frame);
    assert_eq!(original.positions, styled.positions);
    assert_eq!(styled.colors, colors);
    assert_eq!(styled.needles[0].width, 3.0);
    assert_eq!(original.needles[0].width, super::NEEDLE_WIDTH);
}
