//! Animated presentation typography.
//!
//! Text is laid out once per stage size, then every glyph is drawn with its own
//! alpha and substituted glyph id. This supports fade, neon flicker, glitch and
//! typewriter entrances without relaying out every frame. Numeric blocks can
//! instead use the odometer: each digit wheel starts behind its target digit and
//! bounces onto it on a virtual damped spring, and each wheel is drawn as an
//! analytical motion sweep.

use vello::{
    kurbo::{Affine, BezPath, Point, Rect},
    peniko::{BlendMode, Color, Compose, Fill, Mix},
    Scene,
};

use crate::text::{BlockAlign, Fonts, GlyphAtlas, TextGlyph, TextPaint, TextPlan, UiFace};

use super::super::detection::Spring;
use super::super::Rng;
use super::{color::ScenePalette, stage::Stage};

/// Fraction of the line height a hole text runs past a flush plate edge.
const FLUSH_OVERHANG: f64 = 0.05;

/// Largest bounce of the fastest wheel past its stop, in its own cells.
const STOP_BOUNCE_CELLS: f64 = 0.28;

/// Frequency of the stop bounce, in hertz.
const STOP_BOUNCE_HZ: f64 = 2.6;

/// Damping ratio of the stop bounce. Low enough for two or three visible
/// wobbles before the wheel comes to rest.
const STOP_BOUNCE_DAMPING: f64 = 0.16;

/// Slope of the normalized drive at the spawn. The wheels appear already at
/// full speed, so the drive only ever decelerates.
const DRIVE_START_SLOPE: f64 = 3.0;

/// Cells the most significant wheel turns. Each place below it turns
/// `WHEEL_TRAVEL_STEP` times farther.
const TOP_WHEEL_TRAVEL: f64 = 7.0;

/// How many times farther each place below the most significant turns than the
/// place above it.
const WHEEL_TRAVEL_STEP: f64 = 4.0;

/// Least cells the lowest wheel turns during a spin. It does not depend on the
/// value, so short numbers still start with a fast lowest wheel.
const LOWEST_WHEEL_TRAVEL: f64 = 400.0;

/// Spin seconds, chosen per register and per entrance.
const SPIN_SECONDS_MIN: f32 = 0.85;
const SPIN_SECONDS_SPAN: f32 = 0.3;

/// Nominal time after the stop for the entrance clock. The wheels keep being
/// drawn until the spring comes to rest, so this only has to be close.
const STOP_CLOCK_TAIL: f32 = 0.25;

/// Below this sweep length a digit is drawn as a plain glyph. Kept small so
/// even the slow high wheels keep their analytical exposure blur.
const PLAIN_SMEAR_PIXELS: f64 = 0.2;

/// Longest sweep that is drawn, in line heights. The digit strip repeats every
/// ten cells, so a sweep of one period already averages over the whole digit
/// set; stretching further only costs glyphs without changing the result.
const MAX_SWEEP_CELLS: f64 = 10.0;

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

/// One wheel of a numeric register.
#[derive(Clone, Copy, Debug)]
struct Dial {
    /// Index into the laid-out glyphs.
    glyph: usize,
    /// Digit the wheel stops on.
    target: u32,
    /// Cells the wheel turns during the spin.
    travel: f64,
}

/// The virtual stop spring. It runs on the register progress with zero as its
/// rest value, so its value is the small bounce the wheels make at the stop.
/// The shared progress keeps the gear relation through the bounce.
#[derive(Clone, Copy, Debug)]
struct Bounce {
    spring: Spring,
    stiffness: f32,
    damping: f32,
}

/// Damped angular frequency of the stop bounce, in radians per second.
fn bounce_radians() -> f64 {
    std::f64::consts::TAU * STOP_BOUNCE_HZ
}

impl Bounce {
    fn new() -> Self {
        let zeta = STOP_BOUNCE_DAMPING;
        let omega = bounce_radians() / (1.0 - zeta * zeta).sqrt();
        Self {
            spring: Spring::default(),
            stiffness: (omega * omega) as f32,
            damping: (2.0 * zeta * omega) as f32,
        }
    }

    /// Gives the spring the speed the spin still has when it reaches the stop.
    fn release(&mut self, speed: f32) {
        self.spring.reset(0.0);
        self.spring.impulse(speed);
    }

    fn advance(&mut self, dt: f32) {
        self.spring.advance(dt, self.stiffness, self.damping);
    }

    fn value(&self) -> f32 {
        self.spring.value()
    }

    fn rest(&mut self) {
        self.spring.reset(0.0);
    }

    fn settled(&self, travel: f64) -> bool {
        self.spring
            .is_settled((0.02 / travel) as f32, (0.5 / travel) as f32)
    }
}

/// One number: its wheels share one drive and stop together on one bounce.
struct Register {
    /// Cells the lowest wheel turns. The stop spring's amplitude is measured in
    /// these cells.
    travel: f64,
    dials: Vec<Dial>,
    bounce: Bounce,
    /// Seconds from release to the stop.
    seconds: f32,
    /// Drive slope at the stop, progress per unit of normalized time.
    entry_slope: f64,
    released: bool,
    /// Progress drawn after the previous frame, and its change over the frame.
    shown: f32,
    exposure: f32,
}

impl Register {
    fn advance(&mut self, dt: f32, elapsed: f32) {
        let previous = self.shown;
        let shown = if elapsed >= self.seconds {
            if !self.released {
                self.released = true;
                self.bounce
                    .release((self.entry_slope / f64::from(self.seconds)) as f32);
            }
            self.bounce.advance(dt);
            1.0 + self.bounce.value()
        } else {
            drive_progress(f64::from(elapsed / self.seconds), self.entry_slope) as f32
        };
        self.shown = shown;
        self.exposure = shown - previous;
    }

    fn settled(&self) -> bool {
        self.released && self.bounce.settled(self.travel)
    }
}

/// The spinning digits of one text block.
struct Odometer {
    atlas: GlyphAtlas,
    registers: Vec<Register>,
    /// Spin time of the slowest register plus the clock tail.
    duration: f32,
}

impl Odometer {
    /// Reads digit runs out of a laid-out plan. A decimal point between two
    /// digits joins the run, so `2.3` is one register instead of two.
    fn new(atlas: GlyphAtlas, plan: &TextPlan) -> Self {
        let decimal = atlas.glyph('.');
        let mut runs: Vec<Vec<usize>> = Vec::new();
        let mut run: Vec<usize> = Vec::new();
        for (index, glyph) in plan.glyphs.iter().enumerate() {
            if atlas.digit(glyph.id).is_some() {
                run.push(index);
                continue;
            }
            let joined = decimal == Some(glyph.id)
                && !run.is_empty()
                && plan
                    .glyphs
                    .get(index + 1)
                    .is_some_and(|next| atlas.digit(next.id).is_some());
            if !joined && !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
        }
        if !run.is_empty() {
            runs.push(run);
        }
        let registers = runs
            .iter()
            .map(|run| build_register(&atlas, plan, run))
            .collect();
        let mut odometer = Self {
            atlas,
            registers,
            duration: 0.0,
        };
        odometer.finish();
        odometer
    }

    /// Starts one spin with a fresh length. The wheels of a register start
    /// together.
    fn rewind(&mut self, rng: &mut Rng) {
        let mut duration = 0.0f32;
        for register in &mut self.registers {
            register.exposure = 0.0;
            register.released = false;
            register.bounce.rest();
            register.shown = 0.0;
            register.seconds = SPIN_SECONDS_MIN + rng.unit() * SPIN_SECONDS_SPAN;
            // The stop spring's amplitude is the bounce in cells, so the spin
            // only has to hand it the matching speed.
            let entry_speed = STOP_BOUNCE_CELLS * bounce_radians() / register.travel;
            register.entry_slope = entry_speed * f64::from(register.seconds);
            duration = duration.max(register.seconds + STOP_CLOCK_TAIL);
        }
        self.duration = duration;
    }

    /// Puts every wheel on its stop digit without animating.
    fn finish(&mut self) {
        for register in &mut self.registers {
            register.exposure = 0.0;
            register.shown = 1.0;
            register.released = true;
            register.bounce.rest();
        }
        self.duration = 0.0;
    }

    fn advance(&mut self, dt: f32, elapsed: f32) {
        for register in &mut self.registers {
            register.advance(dt, elapsed);
        }
    }

    fn settled(&self) -> bool {
        self.registers.iter().all(Register::settled)
    }
}

/// Builds one register from a digit run. The most significant wheel turns
/// `TOP_WHEEL_TRAVEL` cells, each place below turns `WHEEL_TRAVEL_STEP` times
/// farther, and the lowest wheel is held to at least `LOWEST_WHEEL_TRAVEL`, so
/// every wheel starts behind its target regardless of the value.
fn build_register(atlas: &GlyphAtlas, plan: &TextPlan, run: &[usize]) -> Register {
    let digits: Vec<u32> = run
        .iter()
        .map(|&index| {
            atlas
                .digit(plan.glyphs[index].id)
                .expect("a register run holds digits")
        })
        .collect();
    let lowest = digits.len() - 1;
    let dials: Vec<Dial> = digits
        .iter()
        .enumerate()
        .map(|(index, &target)| {
            let travel = TOP_WHEEL_TRAVEL * WHEEL_TRAVEL_STEP.powi(index as i32);
            Dial {
                glyph: run[index],
                target,
                travel: if index == lowest {
                    travel.max(LOWEST_WHEEL_TRAVEL)
                } else {
                    travel
                },
            }
        })
        .collect();
    let travel = dials.last().expect("a register holds digits").travel;
    Register {
        travel,
        dials,
        bounce: Bounce::new(),
        seconds: 0.0,
        entry_slope: 0.0,
        released: false,
        shown: 1.0,
        exposure: 0.0,
    }
}

/// Normalized spin drive. It spawns at `DRIVE_START_SLOPE` and decelerates to
/// `entry_slope` when it reaches one, so the wheel is already spinning on its
/// first frame and the stop spring takes over without a velocity jump.
fn drive_progress(u: f64, entry_slope: f64) -> f64 {
    let u2 = u * u;
    let u3 = u2 * u;
    (1.0 + entry_slope) * u3 - (3.0 + entry_slope) * u2 + DRIVE_START_SLOPE * u
}

/// Sweep length actually drawn for one wheel, in its own cells. A wheel that
/// turns more than a full digit period per exposure looks the same as one that
/// turns exactly a period, so the range is capped there.
fn drawn_sweep_cells(exposure: f32, travel: f64) -> f64 {
    (f64::from(exposure.abs()) * travel).min(MAX_SWEEP_CELLS)
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
    odometer_state: Option<Odometer>,
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
            odometer_state: None,
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

    /// Uses the odometer reveal for numeric values. The paint must be a fill
    /// because the spinning digit windows need a solid ink color.
    pub fn with_odometer(mut self, odometer: bool) -> Self {
        self.odometer = odometer;
        self
    }

    /// Drops the odometer so later entrances use the text's character effect.
    pub fn disable_odometer(&mut self) {
        self.odometer = false;
        self.odometer_state = None;
    }

    #[cfg(test)]
    pub fn has_odometer(&self) -> bool {
        self.odometer_state.is_some()
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
        assert!(
            !self.odometer || self.paint == PaintStyle::Fill,
            "an odometer needs a fill paint because its digit windows are solid ink"
        );
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
        self.odometer_state = self
            .odometer
            .then(|| Odometer::new(fonts.glyph_atlas(self.face, size), &plan));
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
        match (&mut self.odometer_state, self.phase) {
            (Some(odometer), TextPhase::Appear) => {
                // A stage change rebuilt the wheels, so the entrance restarts
                // on the new layout instead of freezing mid-spin.
                odometer.rewind(rng);
                self.duration = odometer.duration;
                self.elapsed = 0.0;
            }
            (Some(odometer), _) => odometer.finish(),
            (None, _) => {}
        }
    }

    fn init_motions(&mut self, rng: &mut Rng) {
        let Some(plan) = &self.plan else {
            return;
        };
        let count = plan.glyph_count();
        self.glyphs.clear();
        if let Some(odometer) = &mut self.odometer_state {
            odometer.rewind(rng);
            self.duration = odometer.duration;
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
        // An exit always starts from the settled digits, even when it
        // interrupts a spin.
        if let Some(odometer) = &mut self.odometer_state {
            odometer.finish();
        }
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
        if let Some(odometer) = &mut self.odometer_state {
            odometer.finish();
        }
    }

    pub fn advance(&mut self, dt: f32) {
        if self.phase == TextPhase::Hidden {
            return;
        }
        if self.phase != TextPhase::Visible {
            self.elapsed += dt;
        }
        // The wheels keep ringing out after the phase turns visible; the
        // odometer stays the drawing path until they are at rest.
        if matches!(self.phase, TextPhase::Appear | TextPhase::Visible) {
            if let Some(odometer) = &mut self.odometer_state {
                if !odometer.settled() {
                    odometer.advance(dt, self.elapsed);
                }
            }
        }
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
        if let Some(odometer) = &self.odometer_state {
            let appearing = self.phase == TextPhase::Appear;
            let settling = self.phase == TextPhase::Visible && !odometer.settled();
            if appearing || settling {
                self.draw_odometer(scene, text_transform, palette, alpha, plan, odometer);
                self.draw_caret(scene, text_transform, stage, palette, alpha, time);
                return;
            }
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
        odometer: &Odometer,
    ) {
        let atlas = &odometer.atlas;
        let line_height = plan.line_height.max(1.0);
        // Punctuation, units and spaces do not move; every digit belongs to a
        // register and is drawn as a wheel below.
        for glyph in &plan.glyphs {
            if atlas.digit(glyph.id).is_none() {
                atlas.draw(
                    scene,
                    transform * Affine::translate((glyph.x, glyph.y)),
                    glyph.id,
                    palette.ink,
                    alpha,
                );
            }
        }
        for register in &odometer.registers {
            let progress = f64::from(register.shown);
            for dial in &register.dials {
                let glyph = &plan.glyphs[dial.glyph];
                // The displacement of this wheel during the exposure, in its
                // own cells and in stage units.
                let smear = drawn_sweep_cells(register.exposure, dial.travel) * line_height;
                let line = plan.lines.get(usize::from(glyph.line));
                let top = line.map_or(glyph.y - plan.cap_height, |line| line.top);
                let bottom = line.map_or(glyph.y + line_height, |line| line.top + line.line_height);
                let clip = Rect::new(glyph.x, top, glyph.x + glyph.advance, bottom);
                scene.push_clip_layer(Fill::NonZero, transform, &clip);
                draw_wheel(
                    scene,
                    transform,
                    atlas,
                    plan,
                    glyph,
                    *dial,
                    progress,
                    smear,
                    line_height,
                    palette.ink,
                    alpha,
                );
                scene.pop_layer();
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
            palette.ink.with_alpha(alpha),
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

/// Continuous position of a wheel in its periodic digit strip. It starts one
/// travel behind the target digit and lands exactly on it at full progress.
fn wheel_center(dial: Dial, progress: f64) -> f64 {
    // The strip is periodic in ten cells; keeping the center in one period
    // keeps the drawn range small for registers that turn many revolutions.
    (f64::from(dial.target) - (1.0 - progress) * dial.travel).rem_euclid(10.0)
}

/// Draws one wheel of an odometer cell: the digit strip around the current
/// position, with every cell swept by the exposure displacement.
#[allow(clippy::too_many_arguments)]
fn draw_wheel(
    scene: &mut Scene,
    transform: Affine,
    atlas: &GlyphAtlas,
    plan: &TextPlan,
    glyph: &TextGlyph,
    dial: Dial,
    progress: f64,
    smear: f64,
    line_height: f64,
    color: Color,
    alpha: f32,
) {
    let line = plan.lines.get(usize::from(glyph.line));
    let top = line.map_or(glyph.y - plan.cap_height, |line| line.top);
    let bottom = line.map_or(glyph.y + line_height, |line| line.top + line.line_height);
    let center = wheel_center(dial, progress);
    let half = (smear / line_height * 0.5).ceil() as i64 + 2;
    for value in (center.floor() as i64 - half)..=(center.ceil() as i64 + half) {
        let digit = value.rem_euclid(10) as usize;
        let Some(id) = atlas.digit_id(digit) else {
            continue;
        };
        let Some(ink) = atlas.digit_ink(digit) else {
            continue;
        };
        let offset = (value as f64 - center) * line_height;
        let ink_center = glyph.y + offset - f64::from(ink.center());
        let reach = f64::from(ink.height()) * 0.5 + smear * 0.5;
        if ink_center + reach < top || ink_center - reach > bottom {
            continue;
        }
        let position = transform * Affine::translate((glyph.x, glyph.y + offset));
        if smear < PLAIN_SMEAR_PIXELS {
            atlas.draw(scene, position, id, color, alpha);
        } else {
            atlas.draw_swept(scene, position, id, ink, smear as f32, color, alpha);
        }
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

    fn laid_out(source: &str, rng: &mut Rng) -> anyhow::Result<AnimatedText> {
        let mut fonts = Fonts::new()?;
        let mut text = AnimatedText::new(
            source,
            96.0,
            UiFace::SemiBold,
            BlockAlign::Left,
            None,
            1.02,
            PaintStyle::Fill,
        )
        .with_odometer(true);
        text.layout(&mut fonts, &stage(), rng);
        Ok(text)
    }

    fn dials(text: &AnimatedText) -> Vec<(u32, f64)> {
        let odometer = text
            .odometer_state
            .as_ref()
            .expect("the text has an odometer");
        assert_eq!(odometer.registers.len(), 1, "expected one register");
        odometer.registers[0]
            .dials
            .iter()
            .map(|dial| (dial.target, dial.travel))
            .collect()
    }

    #[test]
    fn odometer_wheels_start_farther_towards_the_lowest_place() -> anyhow::Result<()> {
        let mut rng = Rng::new(3);
        for source in [
            "8 RND", "43 RND", "100 RND", "$499 USD", "4299 MM", "2.3 KG", "0.7 KG",
        ] {
            let text = laid_out(source, &mut rng)?;
            let dials = dials(&text);
            let count = dials.len();
            for (index, (_, travel)) in dials.iter().enumerate() {
                let natural = TOP_WHEEL_TRAVEL * WHEEL_TRAVEL_STEP.powi(index as i32);
                let expected = if index + 1 == count {
                    natural.max(LOWEST_WHEEL_TRAVEL)
                } else {
                    natural
                };
                assert!(
                    (travel - expected).abs() < 1e-9,
                    "{source} wheel {index} turns {travel} cells"
                );
            }
            let (_, lowest) = *dials.last().expect("a number has a wheel");
            assert!(
                lowest >= LOWEST_WHEEL_TRAVEL,
                "{source} lowest wheel turns only {lowest} cells"
            );
            for pair in dials.windows(2) {
                assert!(
                    pair[0].1 < pair[1].1,
                    "{source} a wheel does not turn farther than the one above it"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn odometer_wheels_stop_on_their_target_digits() -> anyhow::Result<()> {
        let mut rng = Rng::new(9);
        for source in ["$499 USD", "4299 MM", "2.3 KG", "0.7 KG"] {
            let text = laid_out(source, &mut rng)?;
            let odometer = text.odometer_state.as_ref().expect("odometer");
            for register in &odometer.registers {
                for dial in &register.dials {
                    let at_stop = wheel_center(*dial, 1.0);
                    assert!(
                        (at_stop - f64::from(dial.target)).abs() < 1e-9,
                        "{source} stops {} cells off target",
                        at_stop - f64::from(dial.target)
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn odometer_stops_on_its_target_with_a_damped_bounce() -> anyhow::Result<()> {
        let mut rng = Rng::new(5);
        for source in ["4299 MM", "2.3 KG", "43 RND"] {
            let mut text = laid_out(source, &mut rng)?;
            text.enter(CharEffect::Fade, &mut rng);
            let travel = text.odometer_state.as_ref().expect("odometer").registers[0].travel;
            let mut peak: f64 = 0.0;
            let mut swings = 0;
            let mut signed = 0i32;
            let mut elapsed = 0.0f32;
            let dt = 1.0 / 120.0;
            loop {
                text.advance(dt);
                elapsed += dt;
                let odometer = text.odometer_state.as_ref().expect("odometer");
                let value = f64::from(odometer.registers[0].shown);
                peak = peak.max((value - 1.0) * travel);
                let sign = if value > 1.0 {
                    1
                } else if value < 1.0 {
                    -1
                } else {
                    0
                };
                if sign != 0 && signed != 0 && sign != signed {
                    swings += 1;
                }
                if sign != 0 {
                    signed = sign;
                }
                if text.phase() == TextPhase::Visible && odometer.settled() {
                    break;
                }
                assert!(elapsed < 2.6, "{source} did not settle");
            }
            let odometer = text.odometer_state.as_ref().expect("odometer");
            let value = f64::from(odometer.registers[0].shown);
            assert!(
                (value - 1.0).abs() * travel < 0.02,
                "{source} stops {} cells off target",
                (value - 1.0).abs() * travel
            );
            assert!(
                peak > 0.15 && peak < STOP_BOUNCE_CELLS * 1.2,
                "{source} bounces {peak} cells past the stop"
            );
            assert!(
                swings >= 3,
                "{source} makes only {swings} swings, not a damped spring"
            );
            assert!(
                elapsed > SPIN_SECONDS_MIN,
                "{source} stopped before the spin finished"
            );
        }
        Ok(())
    }

    #[test]
    fn drive_spawns_at_full_speed_and_only_decelerates() {
        let entry = 0.004;
        assert_eq!(drive_progress(0.0, entry), 0.0);
        assert!((drive_progress(1.0, entry) - 1.0).abs() < 1e-12);
        let probe = 1e-7;
        let start = (drive_progress(probe, entry) - drive_progress(0.0, entry)) / probe;
        assert!(
            (start - DRIVE_START_SLOPE).abs() < 1e-4,
            "the drive spawns at {start}, not at full speed"
        );
        let step = 1e-3;
        let mut previous = start;
        let mut u = step;
        while u < 1.0 {
            let slope = (drive_progress(u, entry) - drive_progress(u - step, entry)) / step;
            assert!(slope <= previous + 1e-5, "the drive speeds up at {u}");
            assert!(slope > 0.0, "the drive runs backwards at {u}");
            previous = slope;
            u += step;
        }
        assert!(
            (previous - entry).abs() < 1e-3,
            "the drive lands at {previous}, not at the entry speed"
        );
    }

    #[test]
    fn drawn_sweep_caps_at_one_digit_period() {
        assert_eq!(drawn_sweep_cells(0.0, 1299.0), 0.0);
        assert!((drawn_sweep_cells(0.001, 1299.0) - 1.299).abs() < 1e-4);
        assert_eq!(drawn_sweep_cells(1.0, 1299.0), MAX_SWEEP_CELLS);
        // A slow wheel is not capped and keeps its exact displacement.
        assert!((drawn_sweep_cells(0.01, 13.0) - 0.13).abs() < 1e-4);
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_odometer_spin_preview() -> anyhow::Result<()> {
        use vello::{util::RenderContext, wgpu, RenderParams};

        let width = 1280u32;
        let height = 1024u32;
        let stage = Stage::new(Size::new(f64::from(width), f64::from(height)));
        let mut fonts = Fonts::new()?;
        let mut rng = Rng::new(7);
        let mut texts = Vec::new();
        for (source, x) in [("4299 MM", 60.0), ("2.3 KG", 560.0), ("43 RND", 960.0)] {
            let mut text = AnimatedText::new(
                source,
                96.0,
                UiFace::SemiBold,
                BlockAlign::Left,
                None,
                1.02,
                PaintStyle::Fill,
            )
            .with_odometer(true);
            text.layout(&mut fonts, &stage, &mut rng);
            text.enter(CharEffect::Fade, &mut rng);
            texts.push((text, x));
        }
        let mut palette = ScenePalette::placeholder();
        palette.ink = Color::from_rgb8(240, 240, 244);
        let frames = [1usize, 3, 8, 20, 60, 96, 108, 124];
        let mut scene = Scene::new();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(10, 11, 16),
            None,
            &Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        );
        let row_height = (f64::from(height) - 48.0) / frames.len() as f64;
        let dt = 1.0f32 / 120.0;
        let mut row = 0;
        for frame in 0..=*frames.last().expect("the preview shows frames") {
            if frames.contains(&frame) {
                let top = 24.0 + row as f64 * row_height;
                for (text, x) in &texts {
                    text.draw(
                        &mut scene,
                        Affine::translate((*x, top)),
                        &stage,
                        palette,
                        1.0,
                        0.0,
                    );
                }
                row += 1;
            }
            for (text, _) in &mut texts {
                text.advance(dt);
            }
        }

        let mut context = RenderContext::new();
        let id = pollster::block_on(crate::render::prepare_device(
            &mut context,
            None,
            wgpu::DeviceDescriptor::default(),
        ))?;
        let device = &context.devices[id];
        let texture = device.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Odometer preview"),
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
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let params = RenderParams {
            base_color: Color::TRANSPARENT,
            width,
            height,
            antialiasing_method: crate::render::ANTIALIASING,
        };
        let mut renderer = crate::render::renderer(&device.device)?;
        renderer.render_to_texture(&device.device, &device.queue, &scene, &view, &params)?;
        let row_bytes = width * 4;
        assert_eq!(row_bytes % 256, 0, "the readback row must be copy aligned");
        let readback = device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Odometer preview readback"),
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
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
            },
            texture.size(),
        );
        device.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let pixels = readback.slice(..).get_mapped_range()?.to_vec();
        readback.unmap();
        if let Some(path) = std::env::var_os("VU_ODOMETER_PREVIEW_PATH") {
            let file = std::fs::File::create(std::path::PathBuf::from(path))?;
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&pixels)?;
        }
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
