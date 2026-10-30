use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};

use vello::{
    kurbo::{Affine, Point, Rect},
    peniko::{Blob, Color, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality},
    util::DeviceHandle,
    wgpu, RenderParams, Renderer, Scene,
};

/// An immutable vector subscene whose raster can be retained across frames.
#[derive(Clone)]
pub struct RasterAsset(Arc<RasterSource>);

struct RasterSource {
    scene: Scene,
    image: ImageData,
    origin: Point,
}

impl PartialEq for RasterAsset {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RasterAsset {}

impl RasterAsset {
    /// Captures a scene inside pixel bounds that include its antialiasing fringe.
    /// Integer cropping retains the original raster grid.
    #[must_use]
    pub fn new(source: &Scene, bounds: Rect) -> Self {
        assert!([bounds.x0, bounds.y0, bounds.x1, bounds.y1]
            .into_iter()
            .all(f64::is_finite));
        let origin = Point::new(bounds.x0.floor(), bounds.y0.floor());
        let width = bounds.x1.ceil() - origin.x;
        let height = bounds.y1.ceil() - origin.y;
        assert!(
            width > 0.0
                && height > 0.0
                && width <= f64::from(u32::MAX)
                && height <= f64::from(u32::MAX),
            "raster asset bounds must fit nonempty pixel dimensions"
        );
        let mut scene = Scene::new();
        scene.append(source, Some(Affine::translate((-origin.x, -origin.y))));
        Self(Arc::new(RasterSource {
            scene,
            image: ImageData {
                // The renderer supplies pixels through its image override, as
                // in Vello's register_texture implementation.
                data: Blob::new(Arc::new(&[])),
                format: ImageFormat::Rgba8,
                // Vello's fine raster stage writes unpremultiplied output.
                alpha_type: ImageAlphaType::Alpha,
                width: width as u32,
                height: height as u32,
            },
            origin,
        }))
    }

    /// Inserts the retained image in draw order and keeps its source alive until
    /// the frame's resource preparation has completed. Prepare these requests
    /// with [`RasterAssetCache::prepare`] before rasterizing the target scene.
    pub fn draw(&self, scene: &mut Scene, requests: &mut Vec<Self>, transform: Affine) {
        let brush = ImageBrush::new(self.0.image.clone()).with_quality(ImageQuality::Low);
        scene.draw_image(
            &brush,
            transform * Affine::translate(self.0.origin.to_vec2()),
        );
        requests.push(self.clone());
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RasterId(u64);

struct CachedRaster {
    source: Weak<RasterSource>,
    image: ImageData,
}

/// GPU realizations of immutable raster assets. Sources own their lifetime;
/// expired assets release their registered texture on the next preparation.
#[derive(Default)]
pub struct RasterAssetCache {
    entries: BTreeMap<RasterId, CachedRaster>,
}

impl RasterAssetCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prepare(
        &mut self,
        renderer: &mut Renderer,
        device: &DeviceHandle,
        requests: &[RasterAsset],
    ) -> anyhow::Result<()> {
        self.entries.retain(|_, entry| {
            if entry.source.strong_count() > 0 {
                true
            } else {
                renderer.unregister_texture(entry.image.clone());
                false
            }
        });
        for asset in requests {
            let source = &asset.0;
            let id = RasterId(source.image.data.id());
            if self.entries.contains_key(&id) {
                // Solid-only renders can replace Vello's image atlas between
                // uses. Refresh its copy from the retained texture.
                renderer.mark_override_image_dirty(&source.image);
                continue;
            }
            let params = RenderParams {
                base_color: Color::TRANSPARENT,
                width: source.image.width,
                height: source.image.height,
                antialiasing_method: super::ANTIALIASING,
            };
            let texture = rasterize_scene(renderer, device, &source.scene, &params)?;
            renderer.override_image(
                &source.image,
                Some(wgpu::TexelCopyTextureInfoBase {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                }),
            );
            self.entries.insert(
                id,
                CachedRaster {
                    source: Arc::downgrade(source),
                    image: source.image.clone(),
                },
            );
        }
        Ok(())
    }
}

pub(super) fn rasterize_scene(
    renderer: &mut Renderer,
    device: &DeviceHandle,
    source: &Scene,
    params: &RenderParams,
) -> anyhow::Result<wgpu::Texture> {
    let texture = device.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Retained vector raster"),
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
    Ok(texture)
}

#[cfg(test)]
#[path = "raster_asset_tests.rs"]
mod tests;
