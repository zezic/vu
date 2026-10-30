use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use parley::{
    fontique::FontInfoOverride,
    style::{FontFamily, FontStyle as ParleyFontStyle, FontWeight, FontWidth, LineHeight},
    Alignment, AlignmentOptions, FontContext, FontData, Layout, LayoutContext,
    PositionedLayoutItem, StyleProperty,
};
use skrifa::prelude::*;
use vello::{
    kurbo::{Affine, Stroke},
    peniko::{Blob, Brush, Color, Fill},
    Glyph, Scene,
};

/// Internal family name for the stripped Iosevka subsets. The original name
/// table is gone, so the family, width, weight, and style are supplied when the
/// fonts are registered.
const UI_FAMILY: &str = "VU Iosevka";

/// Characters the glitch effect can substitute. The bundled subsets contain
/// all printable ASCII.
const GLITCH_CHARS: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!?#%&*+-/<>[]{}()";

/// The two bundled upright Iosevka faces used for the HUD and presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiFace {
    Light,
    SemiBold,
}

impl UiFace {
    fn weight(self) -> FontWeight {
        match self {
            Self::Light => FontWeight::LIGHT,
            Self::SemiBold => FontWeight::SEMI_BOLD,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Light => 0,
            Self::SemiBold => 1,
        }
    }
}

/// Paragraph alignment for wrapped presentation text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlockAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Fill or stroke for a text plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextPaint {
    Fill,
    Stroke(f64),
}

impl Default for TextPaint {
    fn default() -> Self {
        Self::Fill
    }
}

pub struct Fonts {
    fonts: FontContext,
    layouts: LayoutContext<Brush>,
    ui_family: String,
    faces: Vec<FontData>,
    ascii_ids: Vec<Vec<u32>>,
}

impl Fonts {
    pub fn new() -> anyhow::Result<Self> {
        let mut fonts = FontContext::new();
        let mut faces = Vec::with_capacity(2);
        for (bytes, face) in [
            (
                include_bytes!("../assets/fonts/Iosevka/IosevkaTightSoft-Light-subset.ttf")
                    .as_slice(),
                UiFace::Light,
            ),
            (
                include_bytes!("../assets/fonts/Iosevka/IosevkaTightSoft-SemiBold-subset.ttf")
                    .as_slice(),
                UiFace::SemiBold,
            ),
        ] {
            let metadata = FontInfoOverride {
                family_name: Some(UI_FAMILY),
                width: Some(FontWidth::NORMAL),
                style: Some(ParleyFontStyle::Normal),
                weight: Some(face.weight()),
                axes: None,
            };
            let blob = Blob::new(Arc::new(bytes.to_vec()));
            fonts
                .collection
                .register_fonts(blob.clone(), Some(metadata));
            faces.push(FontData::new(blob, 0));
        }
        let ascii_ids = faces.iter().map(|font| ascii_glyph_ids(font)).collect();
        let families: Vec<String> = fonts.collection.family_names().map(str::to_owned).collect();
        if !families.iter().any(|name| name == UI_FAMILY) {
            anyhow::bail!("The bundled Iosevka subsets could not be registered under {UI_FAMILY}");
        }
        Ok(Self {
            fonts,
            layouts: LayoutContext::new(),
            ui_family: UI_FAMILY.to_owned(),
            faces,
            ascii_ids,
        })
    }

    /// Default UI text: upright Iosevka SemiBold for values and labels.
    pub fn layout(&mut self, text: &str, size: f32) -> Text {
        self.layout_ui(text, size, UiFace::SemiBold)
    }

    pub fn layout_ui(&mut self, text: &str, size: f32, face: UiFace) -> Text {
        let family = self.ui_family.clone();
        self.layout_with(text, size, &family, face, None, BlockAlign::Left, None)
    }

    /// Lays out a paragraph that wraps at `max_width`. `line_height` is a
    /// multiple of the font size; pass 1.0 for the font's normal leading.
    pub fn layout_wrapped(
        &mut self,
        text: &str,
        size: f32,
        face: UiFace,
        max_width: Option<f32>,
        align: BlockAlign,
        line_height: f32,
    ) -> Text {
        assert!(size > 0.0, "text needs a positive size");
        assert!(line_height > 0.0, "text needs a positive line height");
        let family = self.ui_family.clone();
        self.layout_with(
            text,
            size,
            &family,
            face,
            max_width,
            align,
            Some(line_height),
        )
    }

    fn layout_with(
        &mut self,
        text: &str,
        size: f32,
        family: &str,
        face: UiFace,
        max_width: Option<f32>,
        align: BlockAlign,
        line_height: Option<f32>,
    ) -> Text {
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(
            Cow::Borrowed(family),
        )));
        builder.push_default(StyleProperty::FontWeight(face.weight()));
        if let Some(line_height) = line_height {
            builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
                line_height,
            )));
        }
        let mut layout = builder.build(text);
        layout.break_all_lines(max_width.filter(|width| *width > 0.0));
        layout.align(
            match align {
                BlockAlign::Left => Alignment::Left,
                BlockAlign::Center => Alignment::Center,
                BlockAlign::Right => Alignment::Right,
            },
            AlignmentOptions {
                align_when_overflowing: true,
            },
        );
        Text { layout, max_width }
    }

    /// A face-sized glyph atlas for glitch substitutions and odometer digits.
    pub fn glyph_atlas(&self, face: UiFace, size: f32) -> GlyphAtlas {
        assert!(size > 0.0, "a glyph atlas needs a positive size");
        GlyphAtlas::new(&self.faces[face.index()], size)
    }

    /// Substitution glyphs for the glitch effect on the given face.
    pub fn glitch_ids(&self, face: UiFace) -> &[u32] {
        &self.ascii_ids[face.index()]
    }
}

fn ascii_glyph_ids(font: &FontData) -> Vec<u32> {
    let Ok(font_ref) = FontRef::from_index(font.data.as_ref(), font.index) else {
        return Vec::new();
    };
    let charmap = font_ref.charmap();
    GLITCH_CHARS
        .chars()
        .filter_map(|character| charmap.map(character))
        .map(GlyphId::to_u32)
        .collect()
}

/// A font face at one size, with ASCII glyph ids and advances.
pub struct GlyphAtlas {
    font: FontData,
    size: f32,
    glyphs: BTreeMap<char, u32>,
    advances: BTreeMap<char, f32>,
    glitch: Vec<u32>,
    digits: [Option<u32>; 10],
}

impl GlyphAtlas {
    fn new(font: &FontData, size: f32) -> Self {
        let Ok(font_ref) = FontRef::from_index(font.data.as_ref(), font.index) else {
            panic!("the bundled font must parse");
        };
        let charmap = font_ref.charmap();
        let metrics = font_ref.glyph_metrics(Size::new(size), LocationRef::default());
        let mut glyphs = BTreeMap::new();
        let mut advances = BTreeMap::new();
        for character in ' '..='~' {
            let Some(id) = charmap.map(character) else {
                continue;
            };
            glyphs.insert(character, id.to_u32());
            let advance = metrics.advance_width(id).unwrap_or(0.0);
            advances.insert(character, advance);
        }
        let glitch = GLITCH_CHARS
            .chars()
            .filter_map(|character| glyphs.get(&character).copied())
            .collect();
        let digits = std::array::from_fn(|index| {
            char::from_digit(index as u32, 10).and_then(|character| glyphs.get(&character).copied())
        });
        Self {
            font: font.clone(),
            size,
            glyphs,
            advances,
            glitch,
            digits,
        }
    }

    pub fn size(&self) -> f32 {
        self.size
    }

    pub fn glyph(&self, character: char) -> Option<u32> {
        self.glyphs.get(&character).copied()
    }

    pub fn advance(&self, character: char) -> f32 {
        self.advances.get(&character).copied().unwrap_or(0.0)
    }

    pub fn glitch_ids(&self) -> &[u32] {
        &self.glitch
    }

    /// The decimal digit represented by a glyph id, when this face has one.
    pub fn digit(&self, glyph: u32) -> Option<u32> {
        self.digits
            .iter()
            .position(|id| *id == Some(glyph))
            .map(|digit| digit as u32)
    }

    pub fn digit_id(&self, digit: usize) -> Option<u32> {
        self.digits.get(digit).copied().flatten()
    }

    pub fn draw(&self, scene: &mut Scene, transform: Affine, glyph: u32, color: Color, alpha: f32) {
        scene
            .draw_glyphs(&self.font)
            .font_size(self.size)
            .transform(transform)
            .brush(color)
            .brush_alpha(alpha)
            .draw(
                Fill::NonZero,
                std::iter::once(Glyph {
                    id: glyph,
                    x: 0.0,
                    y: 0.0,
                }),
            );
    }

    pub fn draw_char(
        &self,
        scene: &mut Scene,
        transform: Affine,
        character: char,
        color: Color,
        alpha: f32,
    ) -> bool {
        let Some(glyph) = self.glyph(character) else {
            return false;
        };
        self.draw(scene, transform, glyph, color, alpha);
        true
    }
}

pub struct Text {
    layout: Layout<Brush>,
    max_width: Option<f32>,
}

/// One fully positioned glyph in a text plan.
#[derive(Clone, Copy, Debug)]
pub struct TextGlyph {
    /// Index into [`TextPlan::runs`].
    pub run: u16,
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub advance: f64,
    /// Index into [`TextPlan::lines`].
    pub line: u16,
}

/// A run of glyphs sharing one font, size, and variation location.
pub struct TextRun {
    pub font: FontData,
    pub font_size: f32,
    pub normalized_coords: Vec<i16>,
    /// Indices into [`TextPlan::glyphs`], in visual order.
    pub glyphs: Vec<u32>,
}

/// One laid-out line in block coordinates.
#[derive(Clone, Copy, Debug)]
pub struct TextLine {
    pub top: f64,
    pub baseline: f64,
    pub width: f64,
    pub line_height: f64,
}

/// A positioned, immutable snapshot of a [`Text`] layout. Building a plan is
/// explicit so the HUD does not pay for it; presentation text keeps its plan
/// for per-glyph animation.
pub struct TextPlan {
    pub runs: Vec<TextRun>,
    pub glyphs: Vec<TextGlyph>,
    pub lines: Vec<TextLine>,
    /// Width available for alignment. Equal to the wrapping width when one was
    /// given, so right- and center-aligned blocks can be placed by their edge.
    pub max_width: f64,
    pub width: f64,
    pub height: f64,
    pub line_height: f64,
    pub cap_height: f64,
}

impl Text {
    pub fn width(&self) -> f64 {
        f64::from(self.layout.width())
    }

    pub fn height(&self) -> f64 {
        f64::from(self.layout.height())
    }

    pub fn line_count(&self) -> usize {
        self.layout.len()
    }

    pub fn line_height(&self) -> f64 {
        self.layout
            .lines()
            .next()
            .map_or(0.0, |line| f64::from(line.metrics().line_height))
    }

    /// Height of capital letters above the baseline. Centering type by its
    /// typographic ascent leaves uppercase labels visibly high because the
    /// bundled Iosevka faces report a tall ascender; this metric is the actual
    /// optical top of a capital.
    pub fn cap_height(&self) -> f64 {
        self.layout
            .lines()
            .next()
            .and_then(|line| {
                line.items().find_map(|item| match item {
                    PositionedLayoutItem::GlyphRun(glyph_run) => {
                        glyph_run.run().metrics().cap_height.map(f64::from)
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| {
                0.72 * self
                    .layout
                    .lines()
                    .next()
                    .map_or(0.0, |line| f64::from(line.metrics().ascent))
            })
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
        let line = self.layout.lines().next().expect("labels contain one line");
        let transform = transform
            * Affine::translate((
                -(self.layout.width() as f64) / 2.0,
                -f64::from(line.metrics().baseline),
            ));
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                unreachable!("labels contain text only")
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

    /// Captures every positioned glyph and line for animated drawing.
    pub fn plan(&self) -> TextPlan {
        let mut runs = Vec::new();
        let mut glyphs = Vec::new();
        let mut lines = Vec::new();
        let mut cap_height = 0.0;
        for (line_index, line) in self.layout.lines().enumerate() {
            let metrics = line.metrics();
            lines.push(TextLine {
                top: f64::from(metrics.block_min_coord),
                baseline: f64::from(metrics.baseline),
                width: f64::from(metrics.advance - metrics.trailing_whitespace),
                line_height: f64::from(metrics.line_height),
            });
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let run_index = u16::try_from(runs.len()).expect("a text plan has few runs");
                let mut run_glyphs = Vec::new();
                for glyph in glyph_run.positioned_glyphs() {
                    run_glyphs
                        .push(u32::try_from(glyphs.len()).expect("a text plan has few glyphs"));
                    glyphs.push(TextGlyph {
                        run: run_index,
                        id: glyph.id,
                        x: f64::from(glyph.x),
                        y: f64::from(glyph.y),
                        advance: f64::from(glyph.advance),
                        line: u16::try_from(line_index).expect("a text plan has few lines"),
                    });
                }
                if cap_height == 0.0 {
                    cap_height = run.metrics().cap_height.map_or(0.0, f64::from);
                }
                runs.push(TextRun {
                    font: run.font().clone(),
                    font_size: run.font_size(),
                    normalized_coords: run.normalized_coords().to_vec(),
                    glyphs: run_glyphs,
                });
            }
        }
        if cap_height == 0.0 {
            cap_height = lines
                .first()
                .map_or(0.0, |line| (line.baseline - line.top).max(0.0) * 0.72);
        }
        let line_height = lines.first().map_or(0.0, |line| line.line_height);
        TextPlan {
            runs,
            glyphs,
            lines,
            max_width: self
                .max_width
                .map_or_else(|| f64::from(self.layout.width()), f64::from),
            width: f64::from(self.layout.width()),
            height: f64::from(self.layout.height()),
            line_height,
            cap_height,
        }
    }
}

impl TextPlan {
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    pub fn glyph(&self, index: usize) -> Option<&TextGlyph> {
        self.glyphs.get(index)
    }

    /// The caret position after `glyph_index` glyphs, in block coordinates.
    pub fn caret(&self, glyph_index: usize) -> Option<(f64, f64, f64)> {
        let glyph = self.glyphs.get(glyph_index)?;
        Some((
            glyph.x + glyph.advance,
            glyph.y,
            f64::from(self.runs[glyph.run as usize].font_size),
        ))
    }

    pub fn draw(&self, scene: &mut Scene, transform: Affine, color: Color, paint: TextPaint) {
        self.draw_alpha(scene, transform, color, 1.0, paint);
    }

    pub fn draw_alpha(
        &self,
        scene: &mut Scene,
        transform: Affine,
        color: Color,
        alpha: f32,
        paint: TextPaint,
    ) {
        for run in &self.runs {
            let glyphs = run.glyphs.iter().map(|&index| {
                let glyph = &self.glyphs[index as usize];
                Glyph {
                    id: glyph.id,
                    x: glyph.x as f32,
                    y: glyph.y as f32,
                }
            });
            self.draw_run(scene, run, transform, color, alpha, paint, glyphs);
        }
    }

    /// Draws glyph by glyph, letting the caller substitute ids and vary alpha.
    /// Returning `None` skips that glyph, which is how the typewriter hides the
    /// not-yet-revealed tail.
    pub fn draw_animated(
        &self,
        scene: &mut Scene,
        transform: Affine,
        paint: TextPaint,
        mut style: impl FnMut(usize, &TextGlyph) -> Option<(u32, Color, f32)>,
    ) {
        for (index, glyph) in self.glyphs.iter().enumerate() {
            let Some((id, glyph_color, alpha)) = style(index, glyph) else {
                continue;
            };
            let run = &self.runs[glyph.run as usize];
            self.draw_run(
                scene,
                run,
                transform,
                glyph_color,
                alpha,
                paint,
                std::iter::once(Glyph {
                    id,
                    x: glyph.x as f32,
                    y: glyph.y as f32,
                }),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &self,
        scene: &mut Scene,
        run: &TextRun,
        transform: Affine,
        color: Color,
        alpha: f32,
        paint: TextPaint,
        glyphs: impl Iterator<Item = Glyph>,
    ) {
        let run = scene
            .draw_glyphs(&run.font)
            .font_size(run.font_size)
            .transform(transform)
            .normalized_coords(&run.normalized_coords)
            .brush(color)
            .brush_alpha(alpha);
        match paint {
            TextPaint::Fill => run.draw(Fill::NonZero, glyphs),
            TextPaint::Stroke(width) => {
                let stroke = Stroke::new(width);
                run.draw(&stroke, glyphs);
            }
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
        for face in [UiFace::Light, UiFace::SemiBold] {
            let text = fonts.layout_ui("0123456789", 20.0, face);
            let (weight, style) = text.font_attributes();
            assert_eq!(weight, face.weight());
            assert_eq!(style, ParleyFontStyle::Normal);
        }
        Ok(())
    }

    #[test]
    fn wrapped_paragraph_breaks_and_respects_width() -> anyhow::Result<()> {
        let mut fonts = Fonts::new()?;
        let text = "The magazine has a ninety-degree personality and a very long \
                    sentence to force wrapping across several measured lines.";
        let layout = fonts.layout_wrapped(
            text,
            18.0,
            UiFace::Light,
            Some(220.0),
            BlockAlign::Left,
            1.15,
        );
        assert!(layout.line_count() > 1);
        assert!(layout.width() <= 220.0 + 0.5);
        assert!(layout.height() > layout.line_height());
        Ok(())
    }

    #[test]
    fn text_plan_positions_every_glyph_and_line() -> anyhow::Result<()> {
        let mut fonts = Fonts::new()?;
        let layout = fonts.layout_wrapped(
            "SMG P90",
            32.0,
            UiFace::SemiBold,
            None,
            BlockAlign::Center,
            1.0,
        );
        let plan = layout.plan();
        assert_eq!(plan.lines.len(), 1);
        assert!(plan.glyph_count() > 4);
        assert!(plan.cap_height > 0.0);
        assert!(plan.glyphs.iter().all(|glyph| glyph.x.is_finite()));
        assert!(plan.glyphs.iter().all(|glyph| glyph.y.is_finite()));
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].glyphs.len(), plan.glyph_count());
        Ok(())
    }

    #[test]
    fn glyph_atlas_maps_ascii_and_glitch_substitutes() -> anyhow::Result<()> {
        let fonts = Fonts::new()?;
        let atlas = fonts.glyph_atlas(UiFace::SemiBold, 40.0);
        assert!(atlas.glyph('A').is_some());
        assert!(atlas.glyph('0').is_some());
        assert!(atlas.advance('0') > 0.0);
        assert!(!atlas.glitch_ids().is_empty());
        assert!(!fonts.glitch_ids(UiFace::SemiBold).is_empty());
        Ok(())
    }
}
