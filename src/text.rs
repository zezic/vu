use std::{borrow::Cow, sync::Arc};

use anyhow::Context;
use parley::{FontContext, Layout, LayoutContext, PositionedLayoutItem, StyleProperty};
use vello::{
    kurbo::Affine,
    peniko::{Blob, Brush, Color, Fill},
    Glyph, Scene,
};

pub struct Fonts {
    fonts: FontContext,
    layouts: LayoutContext<Brush>,
    family: String,
}

impl Fonts {
    pub fn new() -> anyhow::Result<Self> {
        let mut fonts = FontContext::new();
        fonts.collection.register_fonts(
            Blob::new(Arc::new(include_bytes!("../assets/D-DINExp.ttf").to_vec())),
            None,
        );
        let family = fonts
            .collection
            .family_names()
            .next()
            .context("The bundled DIN font could not be loaded")?
            .to_owned();
        Ok(Self {
            fonts,
            layouts: LayoutContext::new(),
            family,
        })
    }

    pub fn layout(&mut self, text: &str, size: f32) -> Text {
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::FontFamily(
            parley::style::FontFamily::Source(Cow::Borrowed(&self.family)),
        ));
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
