use std::{borrow::Cow, sync::Arc};

use anyhow::Context;
use parley::{
    fontique::FontInfoOverride,
    style::{FontFamily, FontStyle as ParleyFontStyle, FontWeight, FontWidth},
    FontContext, Layout, LayoutContext, PositionedLayoutItem, StyleProperty,
};
use vello::{
    kurbo::Affine,
    peniko::{Blob, Brush, Color, Fill},
    Glyph, Scene,
};

/// Internal family name for the stripped Iosevka subsets. The original name
/// table is gone, so the family, width, weight, and style are supplied when the
/// fonts are registered.
const UI_FAMILY: &str = "VU Iosevka";

/// The four bundled Iosevka faces used for everything except gauge markings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiFace {
    Light,
    LightItalic,
    SemiBold,
    SemiBoldItalic,
}

impl UiFace {
    fn weight(self) -> FontWeight {
        match self {
            Self::Light | Self::LightItalic => FontWeight::LIGHT,
            Self::SemiBold | Self::SemiBoldItalic => FontWeight::SEMI_BOLD,
        }
    }

    fn style(self) -> ParleyFontStyle {
        match self {
            Self::Light | Self::SemiBold => ParleyFontStyle::Normal,
            Self::LightItalic | Self::SemiBoldItalic => ParleyFontStyle::Italic,
        }
    }
}

pub struct Fonts {
    fonts: FontContext,
    layouts: LayoutContext<Brush>,
    gauge_family: String,
    ui_family: String,
}

impl Fonts {
    pub fn new() -> anyhow::Result<Self> {
        let mut fonts = FontContext::new();
        fonts.collection.register_fonts(
            Blob::new(Arc::new(include_bytes!("../assets/D-DINExp.ttf").to_vec())),
            None,
        );
        for (bytes, face) in [
            (
                include_bytes!("../assets/fonts/Iosevka/IosevkaTightSoft-Light-subset.ttf")
                    .as_slice(),
                UiFace::Light,
            ),
            (
                include_bytes!("../assets/fonts/Iosevka/IosevkaTightSoft-LightItalic-subset.ttf")
                    .as_slice(),
                UiFace::LightItalic,
            ),
            (
                include_bytes!("../assets/fonts/Iosevka/IosevkaTightSoft-SemiBold-subset.ttf")
                    .as_slice(),
                UiFace::SemiBold,
            ),
            (
                include_bytes!(
                    "../assets/fonts/Iosevka/IosevkaTightSoft-SemiBoldItalic-subset.ttf"
                )
                .as_slice(),
                UiFace::SemiBoldItalic,
            ),
        ] {
            let metadata = FontInfoOverride {
                family_name: Some(UI_FAMILY),
                width: Some(FontWidth::NORMAL),
                style: Some(face.style()),
                weight: Some(face.weight()),
                axes: None,
            };
            fonts
                .collection
                .register_fonts(Blob::new(Arc::new(bytes.to_vec())), Some(metadata));
        }
        let families: Vec<String> = fonts.collection.family_names().map(str::to_owned).collect();
        let gauge_family = families
            .iter()
            .find(|name| name.contains("DIN"))
            .cloned()
            .context("The bundled DIN font could not be loaded")?;
        if !families.iter().any(|name| name == UI_FAMILY) {
            anyhow::bail!("The bundled Iosevka subsets could not be registered under {UI_FAMILY}");
        }
        let ui_family = UI_FAMILY.to_owned();
        Ok(Self {
            fonts,
            layouts: LayoutContext::new(),
            gauge_family,
            ui_family,
        })
    }

    /// Default UI text: upright Iosevka SemiBold for values and labels.
    pub fn layout(&mut self, text: &str, size: f32) -> Text {
        self.layout_ui(text, size, UiFace::SemiBold)
    }

    pub fn layout_ui(&mut self, text: &str, size: f32, face: UiFace) -> Text {
        let family = self.ui_family.clone();
        self.layout_with(text, size, &family, face.weight(), face.style())
    }

    /// Gauge markings and units retain the original DIN face.
    pub fn layout_gauge(&mut self, text: &str, size: f32) -> Text {
        let family = self.gauge_family.clone();
        self.layout_with(
            text,
            size,
            &family,
            FontWeight::NORMAL,
            ParleyFontStyle::Normal,
        )
    }

    fn layout_with(
        &mut self,
        text: &str,
        size: f32,
        family: &str,
        weight: FontWeight,
        style: ParleyFontStyle,
    ) -> Text {
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(
            Cow::Borrowed(family),
        )));
        builder.push_default(StyleProperty::FontWeight(weight));
        builder.push_default(StyleProperty::FontStyle(style));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        Text { layout }
    }
}

pub struct Text {
    layout: Layout<Brush>,
}

impl Text {
    pub fn width(&self) -> f64 {
        f64::from(self.layout.width())
    }

    /// Draws a single-line label with its left edge and baseline at the origin.
    pub fn draw_left(&self, scene: &mut Scene, transform: Affine, color: Color) {
        self.draw(
            scene,
            transform * Affine::translate((self.width() / 2.0, 0.0)),
            color,
        );
    }

    /// Draws a single-line label centered horizontally at the transform's origin,
    /// with its alphabetic baseline at y = 0.
    pub fn draw(&self, scene: &mut Scene, transform: Affine, color: Color) {
        let line = self
            .layout
            .lines()
            .next()
            .expect("meter labels contain one line");
        let transform = transform
            * Affine::translate((
                -(self.layout.width() as f64) / 2.0,
                -f64::from(line.metrics().baseline),
            ));
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                unreachable!("meter labels contain text only")
            };
            let run = glyph_run.run();
            scene
                .draw_glyphs(run.font())
                .font_size(run.font_size())
                .transform(transform)
                .normalized_coords(run.normalized_coords())
                .brush(color)
                .draw(
                    Fill::NonZero,
                    glyph_run.positioned_glyphs().map(|glyph| Glyph {
                        id: glyph.id,
                        x: glyph.x,
                        y: glyph.y,
                    }),
                );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Text {
        fn font_attributes(&self) -> (FontWeight, ParleyFontStyle) {
            let line = self
                .layout
                .lines()
                .next()
                .expect("test labels contain one line");
            let glyph_run = line
                .items()
                .find_map(|item| match item {
                    PositionedLayoutItem::GlyphRun(glyph_run) => Some(glyph_run),
                    _ => None,
                })
                .expect("test labels contain a glyph run");
            let attributes = glyph_run.run().font_attrs();
            (attributes.weight, attributes.style)
        }

        fn glyph_ids(&self) -> Vec<u32> {
            let line = self
                .layout
                .lines()
                .next()
                .expect("test labels contain one line");
            let mut ids = Vec::new();
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    unreachable!("test labels contain text only")
                };
                ids.extend(glyph_run.positioned_glyphs().map(|glyph| glyph.id));
            }
            ids
        }
    }

    #[test]
    fn subset_metadata_restores_every_ui_face() -> anyhow::Result<()> {
        let mut fonts = Fonts::new()?;
        assert_eq!(fonts.ui_family, UI_FAMILY);
        assert!(fonts
            .fonts
            .collection
            .family_names()
            .any(|name| name == UI_FAMILY));
        for face in [
            UiFace::Light,
            UiFace::LightItalic,
            UiFace::SemiBold,
            UiFace::SemiBoldItalic,
        ] {
            let text = fonts.layout_ui("0123456789", 20.0, face);
            let (weight, style) = text.font_attributes();
            assert_eq!(weight, face.weight());
            assert_eq!(style, face.style());
        }
        Ok(())
    }

    #[test]
    fn gauge_markings_keep_the_din_face_while_ui_uses_iosevka() -> anyhow::Result<()> {
        let mut fonts = Fonts::new()?;
        let gauge = fonts.layout_gauge("0123456789+-", 20.0);
        let ui = fonts.layout_ui("0123456789+-", 20.0, UiFace::SemiBold);
        let italic = fonts.layout_ui("0123456789+-", 20.0, UiFace::LightItalic);
        assert!(!gauge.glyph_ids().is_empty());
        assert_ne!(gauge.glyph_ids(), ui.glyph_ids());
        assert!(!italic.glyph_ids().is_empty());
        Ok(())
    }
}
