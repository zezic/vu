//! Animated presentation typography.
//!
//! Text is laid out once per stage size, then every glyph is drawn with its own
//! alpha and substituted glyph id. This supports fade, neon flicker, glitch and
//! typewriter entrances without relaying out every frame.

use vello::{
    kurbo::{Affine, BezPath, Point, Rect},
    peniko::{BlendMode, Color, Compose, Fill, Mix},
    Scene,
};

use crate::text::{BlockAlign, Fonts, GlyphAtlas, TextPaint, TextPlan, UiFace};

use super::super::Rng;
use super::{color::ScenePalette, stage::Stage};

/// Fraction of the line height a hole text runs past a flush plate edge.
const FLUSH_OVERHANG: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPhase {
    Appear,
    Visible,
    Disappear,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharEffect {
    Fade,
    Flicker,
    Glitch,
    Typewriter,
}

impl CharEffect {
    pub fn random(rng: &mut Rng) -> Self {
        match rng.range(3) {
            0 => Self::Fade,
            1 => Self::Flicker,
            _ => Self::Glitch,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintStyle {
    Fill,
    Stroke,
    Hole,
}

#[derive(Clone, Copy, Debug)]
pub struct Backdrop {
    /// Padding in stage units, ordered left, top, right, bottom. The vertical
    /// padding is either symmetric or flush against one edge.
    pub padding: [f64; 4],
    /// Which corners receive a 45 degree cut, ordered top-left, top-right,
    /// bottom-right, bottom-left.
    pub cut: [bool; 4],
    pub cut_size: f64,
    pub fill_alpha: f32,
}

impl Backdrop {
    pub fn random(rng: &mut Rng, text_size: f64) -> Self {
        let side = |rng: &mut Rng| text_size * (0.12 + rng.unit() as f64 * 0.28);
        let left = side(rng);
        let right = side(rng);
        let vertical = side(rng);
        // An uneven gap above and below makes the plate look misaligned, so an
        // off-center plate is flush with the text instead.
        let padding = match rng.range(3) {
            0 => [left, vertical, right, vertical],
            1 => [left, 0.0, right, vertical],
            _ => [left, vertical, right, 0.0],
        };
        let mut cut = [false; 4];
        let count = rng.range(5);
        let mut order = [0, 1, 2, 3];
        for index in (1..4).rev() {
            let swap = rng.range(index + 1);
            order.swap(index, swap);
        }
        for &corner in &order[..count] {
            cut[corner] = true;
        }
        Self {
            padding,
            cut,
            cut_size: text_size * (0.08 + rng.unit() as f64 * 0.22),
            fill_alpha: 0.72 + rng.unit() * 0.2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GlyphMotion {
    delay: f32,
    duration: f32,
    seed: u32,
    swaps: u8,
}

#[derive(Clone, Copy, Debug)]
struct OdometerMotion {
    start_cycles: f32,
    delay: f32,
    duration: f32,
}

pub struct AnimatedText {
    pub source: String,
    /// Font size in stage units.
    pub size: f64,
    pub face: UiFace,
    pub align: BlockAlign,
    /// Wrapping width in stage units.
    pub wrap: Option<f64>,
    pub line_height: f32,
    pub paint: PaintStyle,
    pub backdrop: Option<Backdrop>,
    pub caret: bool,
    pub phase: TextPhase,
    pub elapsed: f32,
    pub duration: f32,
    pub exit_duration: f32,
    effect: CharEffect,
    exit_effect: CharEffect,
    typewriter_cps: f32,
    stage_unit: f64,
    plan: Option<TextPlan>,
    glyphs: Vec<GlyphMotion>,
    glitch: Vec<u32>,
    odometer: bool,
    odometer_atlas: Option<GlyphAtlas>,
    odometer_motions: Vec<OdometerMotion>,
}

impl AnimatedText {
    pub fn new(
        source: impl Into<String>,
        size: f64,
        face: UiFace,
        align: BlockAlign,
        wrap: Option<f64>,
        line_height: f32,
        paint: PaintStyle,
    ) -> Self {
        assert!(size > 0.0, "presentation text needs a positive size");
        assert!(
            line_height > 0.0,
            "presentation text needs a positive line height"
        );
        Self {
            source: source.into(),
            size,
            face,
            align,
            wrap,
            line_height,
            paint,
            backdrop: None,
            caret: false,
            phase: TextPhase::Hidden,
            elapsed: 0.0,
            duration: 0.0,
            exit_duration: 0.0,
            effect: CharEffect::Fade,
            exit_effect: CharEffect::Fade,
            typewriter_cps: 34.0,
            stage_unit: f64::NAN,
            plan: None,
            glyphs: Vec::new(),
            glitch: Vec::new(),
            odometer: false,
            odometer_atlas: None,
            odometer_motions: Vec::new(),
        }
    }

    pub fn with_backdrop(mut self, backdrop: Backdrop) -> Self {
        self.paint = PaintStyle::Hole;
        self.backdrop = Some(backdrop);
        self
    }

    pub fn with_caret(mut self, caret: bool) -> Self {
        self.caret = caret;
        self
    }

    /// Uses the odometer reveal for numeric values. The paint is forced to a
    /// fill because the spinning digit windows need a solid ink color.
    pub fn with_odometer(mut self, odometer: bool) -> Self {
        self.odometer = odometer;
        self
    }

    pub fn plan(&self) -> Option<&TextPlan> {
        self.plan.as_ref()
    }

    /// Lays out the text for the current stage. Resizing the window or the
    /// stage's reference scale rebuilds the plan; otherwise this is a no-op.
    pub fn layout(&mut self, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        if self.plan.is_some() && (self.stage_unit - stage.unit).abs() <= f64::EPSILON {
            return;
        }
        let size = (self.size * stage.unit).max(1.0) as f32;
        let wrap = self.wrap.map(|wrap| (wrap * stage.unit).max(1.0) as f32);
        let text = fonts.layout_wrapped(
            &self.source,
            size,
            self.face,
            wrap,
            self.align,
            self.line_height,
        );
        let plan = text.plan();
        let glyph_count = plan.glyph_count();
        let previous_count = self.plan.as_ref().map_or(0, TextPlan::glyph_count);
        self.glitch = fonts.glitch_ids(self.face).to_vec();
        self.odometer_atlas = if self.odometer {
            Some(fonts.glyph_atlas(self.face, size))
        } else {
            None
        };
        self.plan = Some(plan);
        self.stage_unit = stage.unit;
        if glyph_count != previous_count || self.glyphs.len() != glyph_count {
            self.init_motions(rng);
            if self.phase == TextPhase::Disappear {
                self.phase = TextPhase::Hidden;
            } else if self.phase != TextPhase::Hidden {
                self.phase = TextPhase::Visible;
            }
        }
    }

    fn init_motions(&mut self, rng: &mut Rng) {
        let Some(plan) = &self.plan else {
            return;
        };
        let count = plan.glyph_count();
        self.glyphs.clear();
        if self.odometer {
            self.odometer_motions.clear();
            self.odometer_motions.reserve(count);
            let mut total = 0.0f32;
            for _ in 0..count {
                let start_cycles = 2.0 + rng.unit() * 3.0;
                let delay = rng.unit() * 0.12;
                let duration = 0.7 + rng.unit() * 0.75;
                total = total.max(delay + duration);
                self.odometer_motions.push(OdometerMotion {
                    start_cycles,
                    delay,
                    duration,
                });
            }
            self.duration = total + 0.08;
            return;
        }
        match self.effect {
            CharEffect::Typewriter => {
                self.typewriter_cps = 28.0 + rng.unit() * 26.0;
                self.duration = count as f32 / self.typewriter_cps + 0.12;
            }
            CharEffect::Fade => {
                let mut total = 0.0f32;
                self.glyphs.reserve(count);
                for _ in 0..count {
                    let delay = rng.unit() * 0.24;
                    let duration = 0.24 + rng.unit() * 0.42;
                    total = total.max(delay + duration);
                    self.glyphs.push(GlyphMotion {
                        delay,
                        duration,
                        seed: rng.next() as u32,
                        swaps: 0,
                    });
                }
                self.duration = total + 0.05;
            }
            CharEffect::Flicker => {
                let mut total = 0.0f32;
                self.glyphs.reserve(count);
                for _ in 0..count {
                    let delay = rng.unit() * 0.2;
                    let duration = 0.22 + rng.unit() * 0.36;
                    total = total.max(delay + duration);
                    self.glyphs.push(GlyphMotion {
                        delay,
                        duration,
                        seed: rng.next() as u32,
                        swaps: 0,
                    });
                }
                self.duration = total + 0.05;
            }
            CharEffect::Glitch => {
                let mut total = 0.0f32;
                self.glyphs.reserve(count);
                for _ in 0..count {
                    let delay = rng.unit() * 0.22;
                    let duration = 0.28 + rng.unit() * 0.36;
                    total = total.max(delay + duration);
                    self.glyphs.push(GlyphMotion {
                        delay,
                        duration,
                        seed: rng.next() as u32,
                        swaps: 1 + rng.range(4) as u8,
                    });
                }
                self.duration = total + 0.05;
            }
        }
    }

    /// Starts the entrance with a freshly chosen effect.
    pub fn enter(&mut self, effect: CharEffect, rng: &mut Rng) {
        assert!(
            self.plan.is_some(),
            "presentation text must be laid out before it enters"
        );
        self.effect = effect;
        self.phase = TextPhase::Appear;
        self.elapsed = 0.0;
        self.init_motions(rng);
    }

    /// Starts the exit. The caller chooses the exit effect independently.
    pub fn leave(&mut self, effect: CharEffect, rng: &mut Rng) {
        assert!(
            self.plan.is_some(),
            "presentation text must be laid out before it leaves"
        );
        self.exit_effect = effect;
        self.phase = TextPhase::Disappear;
        self.elapsed = 0.0;
        self.init_exit_motions(rng);
    }

    fn init_exit_motions(&mut self, rng: &mut Rng) {
        let Some(plan) = &self.plan else {
            return;
        };
        let count = plan.glyph_count();
        self.glyphs.clear();
        match self.exit_effect {
            CharEffect::Typewriter => {
                self.typewriter_cps = 34.0 + rng.unit() * 24.0;
                self.exit_duration = count as f32 / self.typewriter_cps + 0.1;
            }
            CharEffect::Fade | CharEffect::Flicker | CharEffect::Glitch => {
                let mut total = 0.0f32;
                self.glyphs.reserve(count);
                for _ in 0..count {
                    let delay = rng.unit() * 0.16;
                    let duration = if self.exit_effect == CharEffect::Flicker {
                        0.16 + rng.unit() * 0.28
                    } else {
                        0.2 + rng.unit() * 0.34
                    };
                    total = total.max(delay + duration);
                    self.glyphs.push(GlyphMotion {
                        delay,
                        duration,
                        seed: rng.next() as u32,
                        swaps: 1 + rng.range(3) as u8,
                    });
                }
                self.exit_duration = total + 0.04;
            }
        }
    }

    pub fn set_visible(&mut self) {
        self.phase = TextPhase::Visible;
        self.elapsed = self.duration;
    }

    pub fn advance(&mut self, dt: f32) {
        if !matches!(self.phase, TextPhase::Appear | TextPhase::Disappear) {
            return;
        }
        self.elapsed += dt;
        match self.phase {
            TextPhase::Appear if self.elapsed >= self.duration => {
                self.phase = TextPhase::Visible;
                self.elapsed = self.duration;
            }
            TextPhase::Disappear if self.elapsed >= self.exit_duration => {
                self.phase = TextPhase::Hidden;
                self.elapsed = self.exit_duration;
            }
            _ => {}
        }
    }

    pub fn phase(&self) -> TextPhase {
        self.phase
    }

    pub fn is_hidden(&self) -> bool {
        self.phase == TextPhase::Hidden
    }

    pub fn bounds(&self, stage: &Stage) -> Rect {
        let Some(plan) = &self.plan else {
            return Rect::ZERO;
        };
        let padding = self.backdrop_padding(stage);
        let left = padding.map_or(0.0, |padding| padding[0]);
        let right = padding.map_or(0.0, |padding| padding[2]);
        let (top, bottom) = self.plate_vertical(stage);
        Rect::new(0.0, 0.0, plan.max_width + left + right, bottom - top)
    }

    /// Padding in stage units for text punched out of a backdrop.
    fn backdrop_padding(&self, stage: &Stage) -> Option<[f64; 4]> {
        match (&self.paint, &self.backdrop) {
            (PaintStyle::Hole, Some(backdrop)) => Some([
                stage.px(backdrop.padding[0]),
                stage.px(backdrop.padding[1]),
                stage.px(backdrop.padding[2]),
                stage.px(backdrop.padding[3]),
            ]),
            _ => None,
        }
    }

    /// Vertical plate extents in layout coordinates. A flush side meets the
    /// glyph ink, not the line box, which is taller than the symbols.
    fn plate_vertical(&self, stage: &Stage) -> (f64, f64) {
        let Some(plan) = &self.plan else {
            return (0.0, 0.0);
        };
        let Some([_, top, _, bottom]) = self.backdrop_padding(stage) else {
            return (0.0, plan.height);
        };
        let plate_top = if top > 0.0 { -top } else { plan.ink_top };
        let plate_bottom = if bottom > 0.0 {
            plan.height + bottom
        } else {
            plan.ink_bottom
        };
        (plate_top, plate_bottom)
    }

    fn text_origin(&self, stage: &Stage) -> Point {
        let padding = self.backdrop_padding(stage);
        let left = padding.map_or(0.0, |padding| padding[0]);
        let (plate_top, _) = self.plate_vertical(stage);
        let mut y = -plate_top;
        if let (Some([_, top, _, bottom]), Some(plan)) = (padding, &self.plan) {
            // A flush edge lets the hole run slightly past the plate so no
            // antialiasing sliver is left between the text and the surface.
            let overhang = plan.line_height * FLUSH_OVERHANG;
            if top == 0.0 {
                y -= overhang;
            } else if bottom == 0.0 {
                y += overhang;
            }
        }
        Point::new(left, y)
    }

    pub fn draw(
        &self,
        scene: &mut Scene,
        transform: Affine,
        stage: &Stage,
        palette: ScenePalette,
        alpha: f32,
        time: f32,
    ) {
        let Some(plan) = &self.plan else {
            return;
        };
        if self.phase == TextPhase::Hidden || alpha <= 0.001 || plan.glyph_count() == 0 {
            return;
        }
        let text_transform = transform
            * Affine::translate({
                let origin = self.text_origin(stage);
                (origin.x, origin.y)
            });
        if self.odometer && self.phase == TextPhase::Appear {
            self.draw_odometer(scene, text_transform, palette, alpha, plan);
            self.draw_caret(scene, text_transform, stage, palette, alpha, time);
            return;
        }
        match self.paint {
            PaintStyle::Fill => {
                if self.phase == TextPhase::Visible {
                    plan.draw_alpha(scene, text_transform, palette.ink, alpha, TextPaint::Fill);
                } else {
                    plan.draw_animated(scene, text_transform, TextPaint::Fill, |index, glyph| {
                        self.glyph_style(index, glyph.id, time, alpha, palette.ink)
                    });
                }
            }
            PaintStyle::Stroke => {
                let width = (self.size * stage.unit * 0.018).max(0.6);
                if self.phase == TextPhase::Visible {
                    plan.draw_alpha(
                        scene,
                        text_transform,
                        palette.ink,
                        alpha,
                        TextPaint::Stroke(width),
                    );
                } else {
                    plan.draw_animated(
                        scene,
                        text_transform,
                        TextPaint::Stroke(width),
                        |index, glyph| self.glyph_style(index, glyph.id, time, alpha, palette.ink),
                    );
                }
            }
            PaintStyle::Hole => self.draw_hole(scene, transform, stage, palette, alpha, time),
        }
        self.draw_caret(scene, text_transform, stage, palette, alpha, time);
    }

    fn draw_odometer(
        &self,
        scene: &mut Scene,
        transform: Affine,
        palette: ScenePalette,
        alpha: f32,
        plan: &TextPlan,
    ) {
        let Some(atlas) = &self.odometer_atlas else {
            return;
        };
        let line_height = plan.line_height.max(1.0);
        for (index, glyph) in plan.glyphs.iter().enumerate() {
            if let Some(target) = atlas.digit(glyph.id) {
                let motion = self
                    .odometer_motions
                    .get(index)
                    .copied()
                    .unwrap_or(OdometerMotion {
                        start_cycles: 3.0,
                        delay: 0.0,
                        duration: 1.0,
                    });
                let progress = if self.phase == TextPhase::Visible {
                    1.0
                } else {
                    smoothstep(((self.elapsed - motion.delay) / motion.duration).clamp(0.0, 1.0))
                };
                let shift =
                    f64::from(1.0 - progress) * f64::from(motion.start_cycles) * 10.0 * line_height;
                let line = plan.lines.get(usize::from(glyph.line));
                let top = line.map_or(glyph.y - plan.cap_height, |line| line.top);
                let bottom = line.map_or(glyph.y + line_height, |line| line.top + line.line_height);
                let clip = Rect::new(glyph.x, top, glyph.x + glyph.advance, bottom);
                scene.push_clip_layer(Fill::NonZero, transform, &clip);
                let center = f64::from(target) - shift / line_height;
                let first = center.floor() as i32 - 12;
                let last = center.ceil() as i32 + 12;
                for value in first..=last {
                    let digit = value.rem_euclid(10) as usize;
                    let Some(id) = atlas.digit_id(digit) else {
                        continue;
                    };
                    let offset = (f64::from(value) - f64::from(target)) * line_height + shift;
                    if offset.abs() > line_height * 3.0 {
                        continue;
                    }
                    atlas.draw(
                        scene,
                        transform * Affine::translate((glyph.x, glyph.y + offset)),
                        id,
                        palette.ink,
                        alpha,
                    );
                }
                scene.pop_layer();
            } else {
                atlas.draw(
                    scene,
                    transform * Affine::translate((glyph.x, glyph.y)),
                    glyph.id,
                    palette.ink,
                    alpha,
                );
            }
        }
    }

    fn draw_hole(
        &self,
        scene: &mut Scene,
        transform: Affine,
        stage: &Stage,
        palette: ScenePalette,
        alpha: f32,
        time: f32,
    ) {
        let Some(plan) = &self.plan else {
            return;
        };
        let Some(backdrop) = &self.backdrop else {
            return;
        };
        let bounds = self.bounds(stage);
        let cut = stage.px(backdrop.cut_size);
        let path = backdrop_path(bounds, backdrop.cut, cut);
        scene.fill(
            Fill::NonZero,
            transform,
            palette.muted.with_alpha(backdrop.fill_alpha * alpha),
            None,
            &path,
        );
        scene.push_layer(
            Fill::NonZero,
            BlendMode {
                mix: Mix::Normal,
                compose: Compose::DestOut,
            },
            1.0,
            transform,
            &path,
        );
        let text_transform = transform
            * Affine::translate({
                let origin = self.text_origin(stage);
                (origin.x, origin.y)
            });
        if self.phase == TextPhase::Visible {
            plan.draw_alpha(scene, text_transform, Color::BLACK, alpha, TextPaint::Fill);
        } else {
            plan.draw_animated(scene, text_transform, TextPaint::Fill, |index, glyph| {
                self.glyph_style(index, glyph.id, time, alpha, Color::BLACK)
            });
        }
        scene.pop_layer();
    }

    fn glyph_style(
        &self,
        index: usize,
        target: u32,
        time: f32,
        alpha: f32,
        color: Color,
    ) -> Option<(u32, Color, f32)> {
        if matches!(self.phase, TextPhase::Visible) {
            return Some((target, color, alpha));
        }
        let appearing = self.phase == TextPhase::Appear;
        let effect = if appearing {
            self.effect
        } else {
            self.exit_effect
        };
        let total = if appearing {
            self.duration
        } else {
            self.exit_duration
        };
        if total <= 0.0 {
            return None;
        }
        if effect == CharEffect::Typewriter {
            let revealed = (self.elapsed * self.typewriter_cps) as usize;
            let visible = if appearing {
                index < revealed
            } else {
                let remaining = (total - self.elapsed).max(0.0) * self.typewriter_cps;
                index < remaining as usize
            };
            return visible.then_some((target, color, alpha));
        }
        let motion = *self.glyphs.get(index)?;
        match effect {
            CharEffect::Typewriter => unreachable!("handled before the motion lookup"),
            CharEffect::Fade => {
                let local = ((self.elapsed - motion.delay) / motion.duration).clamp(0.0, 1.0);
                let fade = if appearing {
                    smoothstep(local)
                } else {
                    1.0 - smoothstep(local)
                };
                (fade > 0.001).then_some((target, color, alpha * fade))
            }
            CharEffect::Flicker => {
                let local = ((self.elapsed - motion.delay) / motion.duration).clamp(0.0, 1.0);
                let threshold = if appearing {
                    smoothstep(local)
                } else {
                    1.0 - smoothstep(local)
                };
                let bucket = (time * 34.0) as u32;
                let on = hash(motion.seed, bucket) % 1000 < (threshold * 1000.0) as u32;
                let flicker = if on { 1.0 } else { 0.06 };
                Some((target, color, alpha * flicker))
            }
            CharEffect::Glitch => {
                let local = ((self.elapsed - motion.delay) / motion.duration).clamp(0.0, 1.0);
                if local <= 0.0 {
                    return None;
                }
                let settled = smoothstep(local);
                let fade = if appearing { settled } else { 1.0 - settled };
                if fade <= 0.001 {
                    return None;
                }
                let glitch = if appearing { 1.0 - settled } else { settled };
                let bucket = (time * 22.0) as u32;
                let swap = !self.glitch.is_empty()
                    && hash(motion.seed ^ 0x517c_c1b7, bucket) % 1000 < (glitch * 900.0) as u32;
                let id = if swap {
                    let pick = hash(
                        motion.seed ^ 0x9e37_79b9,
                        bucket.wrapping_add(u32::from(motion.swaps)),
                    ) as usize
                        % self.glitch.len();
                    self.glitch[pick]
                } else {
                    target
                };
                Some((id, color, alpha * fade))
            }
        }
    }

    fn draw_caret(
        &self,
        scene: &mut Scene,
        transform: Affine,
        stage: &Stage,
        palette: ScenePalette,
        alpha: f32,
        time: f32,
    ) {
        if !self.caret || self.effect != CharEffect::Typewriter {
            return;
        }
        if !matches!(self.phase, TextPhase::Appear | TextPhase::Visible) {
            return;
        }
        let Some(plan) = &self.plan else {
            return;
        };
        let revealed = (self.elapsed * self.typewriter_cps) as usize;
        if revealed >= plan.glyph_count() {
            return;
        }
        let Some((x, baseline, _)) = plan.caret(revealed) else {
            return;
        };
        let blink = ((time * 3.6).sin() * 0.5 + 0.5).powf(2.0);
        let width = (stage.unit * 4.0).max(1.0);
        let top = baseline - plan.cap_height;
        let bottom = baseline + plan.line_height * 0.18;
        scene.fill(
            Fill::NonZero,
            transform,
            palette.accent.with_alpha(alpha * (0.25 + blink * 0.75)),
            None,
            &Rect::new(x, top, x + width, bottom),
        );
    }
}

fn backdrop_path(bounds: Rect, cut: [bool; 4], cut_size: f64) -> BezPath {
    let mut path = BezPath::new();
    let x0 = bounds.x0;
    let y0 = bounds.y0;
    let x1 = bounds.x1;
    let y1 = bounds.y1;
    let size = cut_size.max(0.0);
    let tl = if cut[0] { size } else { 0.0 };
    let tr = if cut[1] { size } else { 0.0 };
    let br = if cut[2] { size } else { 0.0 };
    let bl = if cut[3] { size } else { 0.0 };
    path.move_to((x0 + tl, y0));
    path.line_to((x1 - tr, y0));
    if tr > 0.0 {
        path.line_to((x1, y0 + tr));
    }
    path.line_to((x1, y1 - br));
    if br > 0.0 {
        path.line_to((x1 - br, y1));
    }
    path.line_to((x0 + bl, y1));
    if bl > 0.0 {
        path.line_to((x0, y1 - bl));
    }
    path.line_to((x0, y0 + tl));
    if tl > 0.0 {
        path.line_to((x0 + tl, y0));
    }
    path.close_path();
    path
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn hash(seed: u32, value: u32) -> u32 {
    let mut x = seed ^ value.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Fonts;
    use vello::kurbo::Size;

    fn stage() -> Stage {
        Stage::new(Size::new(1920.0, 1080.0))
    }

    #[test]
    fn backdrop_vertical_padding_is_centered_or_flush() {
        let mut rng = Rng::new(23);
        for _ in 0..256 {
            let backdrop = Backdrop::random(&mut rng, 96.0);
            let [_, top, _, bottom] = backdrop.padding;
            assert!(
                top == bottom || top == 0.0 || bottom == 0.0,
                "vertical padding {top}/{bottom} leaves the text floating"
            );
        }
    }

    #[test]
    fn flush_backdrop_edges_overhang_the_glyph_ink() -> anyhow::Result<()> {
        let stage = stage();
        let mut fonts = Fonts::new()?;
        let mut rng = Rng::new(31);
        let text = |padding| {
            AnimatedText::new(
                "HANDGUARD",
                64.0,
                UiFace::SemiBold,
                BlockAlign::Left,
                None,
                1.0,
                PaintStyle::Hole,
            )
            .with_backdrop(Backdrop {
                padding,
                cut: [false; 4],
                cut_size: 0.0,
                fill_alpha: 0.8,
            })
        };

        let mut top_flush = text([10.0, 0.0, 10.0, 8.0]);
        top_flush.layout(&mut fonts, &stage, &mut rng);
        let plan = top_flush.plan().expect("the text has been laid out");
        let origin = top_flush.text_origin(&stage);
        let overhang = plan.line_height * FLUSH_OVERHANG;
        assert!(
            (origin.y + plan.ink_top + overhang).abs() < 1e-9,
            "the top edge is {} away from the glyph ink",
            origin.y + plan.ink_top
        );

        let mut bottom_flush = text([10.0, 8.0, 10.0, 0.0]);
        bottom_flush.layout(&mut fonts, &stage, &mut rng);
        let plan = bottom_flush.plan().expect("the text has been laid out");
        let origin = bottom_flush.text_origin(&stage);
        let bounds = bottom_flush.bounds(&stage);
        let overhang = plan.line_height * FLUSH_OVERHANG;
        assert!(
            (origin.y + plan.ink_bottom - bounds.height() - overhang).abs() < 1e-9,
            "the bottom edge is {} away from the glyph ink",
            origin.y + plan.ink_bottom - bounds.height()
        );
        Ok(())
    }

    #[test]
    fn odometer_and_char_effects_settle_into_visible_text() -> anyhow::Result<()> {
        let mut fonts = Fonts::new()?;
        let stage = stage();
        let mut rng = Rng::new(7);
        let mut text = AnimatedText::new(
            "4.3 KG",
            64.0,
            UiFace::SemiBold,
            BlockAlign::Left,
            None,
            1.02,
            PaintStyle::Fill,
        )
        .with_odometer(true);
        text.layout(&mut fonts, &stage, &mut rng);
        assert!(text.bounds(&stage).width() > 0.0);
        text.enter(CharEffect::Glitch, &mut rng);
        assert_eq!(text.phase(), TextPhase::Appear);
        text.advance(text.duration);
        assert_eq!(text.phase(), TextPhase::Visible);
        Ok(())
    }

    #[test]
    fn fade_and_typewriter_finish_after_their_duration() -> anyhow::Result<()> {
        let mut fonts = Fonts::new()?;
        let stage = stage();
        let mut rng = Rng::new(11);
        let mut fade = AnimatedText::new(
            "A representative magazine",
            20.0,
            UiFace::Light,
            BlockAlign::Left,
            Some(300.0),
            1.2,
            PaintStyle::Fill,
        );
        fade.layout(&mut fonts, &stage, &mut rng);
        fade.enter(CharEffect::Fade, &mut rng);
        fade.advance(fade.duration * 0.5);
        assert_eq!(fade.phase(), TextPhase::Appear);
        fade.advance(fade.duration);
        assert_eq!(fade.phase(), TextPhase::Visible);

        let mut typed = AnimatedText::new(
            "A representative magazine or feed capacity is 30 rounds.",
            18.0,
            UiFace::Light,
            BlockAlign::Left,
            Some(280.0),
            1.15,
            PaintStyle::Fill,
        )
        .with_caret(true);
        typed.layout(&mut fonts, &stage, &mut rng);
        typed.enter(CharEffect::Typewriter, &mut rng);
        assert_eq!(typed.phase(), TextPhase::Appear);
        typed.advance(typed.duration);
        assert_eq!(typed.phase(), TextPhase::Visible);
        Ok(())
    }

    #[test]
    fn exit_animation_hides_the_block() -> anyhow::Result<()> {
        let mut fonts = Fonts::new()?;
        let stage = stage();
        let mut rng = Rng::new(19);
        let mut text = AnimatedText::new(
            "HANDGUARD",
            56.0,
            UiFace::SemiBold,
            BlockAlign::Left,
            None,
            1.0,
            PaintStyle::Stroke,
        );
        text.layout(&mut fonts, &stage, &mut rng);
        text.enter(CharEffect::Glitch, &mut rng);
        text.advance(text.duration);
        assert_eq!(text.phase(), TextPhase::Visible);
        text.leave(CharEffect::Flicker, &mut rng);
        text.advance(text.exit_duration);
        assert!(text.is_hidden());
        Ok(())
    }
}
