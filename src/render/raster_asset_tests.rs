use super::*;
use std::sync::mpsc;
use vello::{
    kurbo::{Line, Point, Stroke},
    peniko::{Color, Fill},
    util::RenderContext,
    RenderParams,
};

#[test]
#[ignore = "requires a compute-capable GPU"]
fn cached_raster_preserves_overlapping_translucent_strokes() -> anyhow::Result<()> {
    let width = 256;
    let height = 192;
    let mut context = RenderContext::new();
    let id = pollster::block_on(crate::render::prepare_device(
        &mut context,
        None,
        wgpu::DeviceDescriptor::default(),
    ))?;
    let device = &context.devices[id];
    let mut renderer = crate::render::renderer(&device.device)?;
    let mut source = Scene::new();
    source.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(210, 40, 90, 90),
        None,
        &Rect::new(56.3, 44.7, 144.4, 112.2),
    );
    for (start, end, color) in [
        (
            (47.8, 36.9),
            (154.1, 123.2),
            Color::from_rgba8(240, 220, 50, 160),
        ),
        (
            (47.8, 123.2),
            (154.1, 36.9),
            Color::from_rgba8(30, 160, 230, 120),
        ),
        (
            (47.8, 79.8),
            (154.1, 79.8),
            Color::from_rgba8(210, 100, 60, 190),
        ),
    ] {
        source.stroke(
            &Stroke::new(1.35),
            Affine::IDENTITY,
            color,
            None,
            &Line::new(Point::from(start), Point::from(end)),
        );
    }
    let asset = RasterAsset::new(&source, Rect::new(43.25, 31.5, 159.75, 128.25));
    let mut cache = RasterAssetCache::new();

    let texture = device.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Raster asset comparison"),
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
    let view = texture.create_view(&Default::default());
    for background in [
        Color::TRANSPARENT,
        Color::from_rgba8(35, 75, 130, 125),
        Color::from_rgb8(15, 18, 25),
        Color::from_rgb8(240, 235, 220),
    ] {
        for transform in [Affine::IDENTITY, Affine::translate((24.0, 16.0))] {
            let params = RenderParams {
                base_color: background,
                width,
                height,
                antialiasing_method: crate::render::ANTIALIASING,
            };
            let mut direct = Scene::new();
            direct.append(&source, Some(transform));
            renderer.render_to_texture(&device.device, &device.queue, &direct, &view, &params)?;
            let expected = read_pixels(device, &texture)?;

            let mut cached = Scene::new();
            let mut requests = Vec::new();
            asset.draw(&mut cached, &mut requests, transform);
            cache.prepare(&mut renderer, device, &requests)?;
            renderer.render_to_texture(&device.device, &device.queue, &cached, &view, &params)?;
            let actual = read_pixels(device, &texture)?;
            // Vello writes straight alpha; RGB under zero alpha is invisible.
            // Compare associated colors and alpha, including partially covered pixels.
            let max_error = actual
                .chunks_exact(4)
                .zip(expected.chunks_exact(4))
                .flat_map(|(actual, expected)| {
                    let mut errors = [0_u16; 4];
                    for channel in 0..3 {
                        let actual =
                            (u16::from(actual[channel]) * u16::from(actual[3]) + 127) / 255;
                        let expected =
                            (u16::from(expected[channel]) * u16::from(expected[3]) + 127) / 255;
                        errors[channel] = actual.abs_diff(expected);
                    }
                    errors[3] = u16::from(actual[3].abs_diff(expected[3]));
                    errors
                })
                .max()
                .unwrap();
            assert!(max_error <= 2, "cached raster changed a channel by {max_error}/255 on {background:?} at {transform:?}");
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires a compute-capable GPU"]
fn cached_raster_reuses_its_texture_and_releases_expired_assets() -> anyhow::Result<()> {
    let mut context = RenderContext::new();
    let id = pollster::block_on(crate::render::prepare_device(
        &mut context,
        None,
        wgpu::DeviceDescriptor::default(),
    ))?;
    let device = &context.devices[id];
    let mut renderer = crate::render::renderer(&device.device)?;
    let asset = RasterAsset::new(&Scene::new(), Rect::new(0.0, 0.0, 8.0, 8.0));
    let image = asset.0.image.clone();
    let mut cache = RasterAssetCache::new();
    cache.prepare(&mut renderer, device, std::slice::from_ref(&asset))?;
    assert_eq!(cache.entries.len(), 1);
    let texture = renderer
        .override_image(&image, None)
        .expect("the asset registered its texture");
    renderer.override_image(&image, Some(texture.clone()));
    cache.prepare(&mut renderer, device, std::slice::from_ref(&asset))?;
    let reused = renderer
        .override_image(&image, None)
        .expect("the asset remains registered");
    assert_eq!(texture.texture, reused.texture);
    renderer.override_image(&image, Some(reused));
    assert_eq!(cache.entries.len(), 1);

    let retained = asset.clone();
    drop(asset);
    cache.prepare(&mut renderer, device, &[])?;
    assert_eq!(cache.entries.len(), 1);
    drop(retained);
    cache.prepare(&mut renderer, device, &[])?;
    assert!(cache.entries.is_empty());
    assert!(renderer.override_image(&image, None).is_none());
    Ok(())
}

fn read_pixels(device: &DeviceHandle, texture: &wgpu::Texture) -> anyhow::Result<Vec<u8>> {
    let row_bytes = texture.width() * 4;
    let buffer = device.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Raster comparison pixels"),
        size: u64::from(row_bytes * texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut commands = device.device.create_command_encoder(&Default::default());
    commands.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(texture.height()),
            },
        },
        texture.size(),
    );
    device.queue.submit([commands.finish()]);
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
