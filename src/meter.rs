use std::collections::HashMap;

use instant::Instant;
use strum::VariantArray;
use vello::{
    kurbo::{Affine, Circle, Line, Point, Size, Stroke, Triangle},
    peniko::{Color, Compose, Fill, Gradient},
    Scene,
};

use crate::{
    config::PreampDb,
    frame_schedule::FrameRateLimit,
    processor::Processor,
    render::Frame,
    scales::{draw_scale, generate_din_scale, Mark, Scale, ScaleSetting},
    text::{Fonts, Text},
    AudioEvent,
};

const VU_WIDTH: f64 = 320.0;
const VU_HEIGHT: f64 = 220.0;
const CENTER_Y: f64 = 207.0;
const NEEDLE_RADIUS: f64 = 174.0;
const NEEDLE_WIDTH: f64 = 1.0;
const NEEDLE_INK: Color = Color::from_rgb8(255, 200, 160);

mod motion;
use motion::Motion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Horizontal,
    Vertical,
}

impl Layout {
    fn size(self) -> Size {
        match self {
            Self::Horizontal => Size::new(VU_WIDTH * 2.0, VU_HEIGHT),
            Self::Vertical => Size::new(VU_WIDTH, VU_HEIGHT * 2.0),
        }
    }

    /// Channel order is left/right, or top/bottom when stacked.
    fn pivots(self) -> [Point; 2] {
        let first = Point::new(VU_WIDTH / 2.0, CENTER_Y);
        let second = match self {
            Self::Horizontal => Point::new(VU_WIDTH * 1.5, CENTER_Y),
            Self::Vertical => Point::new(VU_WIDTH / 2.0, CENTER_Y + VU_HEIGHT),
        };
        [first, second]
    }

    /// Readout order is preamp/frame limit.
    fn readouts(self) -> [Point; 2] {
        match self {
            Self::Horizontal => [Point::new(VU_WIDTH, 150.0), Point::new(VU_WIDTH, 180.0)],
            Self::Vertical => [
                Point::new(VU_WIDTH / 4.0, CENTER_Y + 10.0),
                Point::new(VU_WIDTH * 0.75, CENTER_Y + 10.0),
            ],
        }
    }
}

/// Keeps the layout and its fitted or user-adjusted transform together.
#[derive(Clone, Copy, PartialEq)]
pub struct View {
    layout: Layout,
    transform: Affine,
    viewport: Size,
}

impl View {
    pub fn fit(viewport: Size) -> Self {
        assert!(
            viewport.width > 0.0 && viewport.height > 0.0,
            "a drawable viewport must have an area"
        );
        // Switching at one face's aspect ratio chooses the larger fitted scales.
        let layout = if viewport.width / viewport.height >= VU_WIDTH / VU_HEIGHT {
            Layout::Horizontal
        } else {
            Layout::Vertical
        };
        let size = layout.size();
        let scale = (viewport.width / size.width).min(viewport.height / size.height);
        let transform = Affine::translate((
            (viewport.width - size.width * scale) / 2.0,
            (viewport.height - size.height * scale) / 2.0,
        )) * Affine::scale(scale);
        Self {
            layout,
            transform,
            viewport,
        }
    }

    pub fn zoom(&mut self, focus: Point, factor: f64) {
        assert!(factor > 0.0, "zoom must preserve a positive scale");
        let point = self.transform.inverse() * focus;
        self.transform *= Affine::translate(point.to_vec2())
            * Affine::scale(factor)
            * Affine::translate(-point.to_vec2());
    }
}

struct Needle {
    overload: f32,
    motion: Motion,
}

struct ScaleReadout {
    setting: ScaleSetting,
    value: f32,
    text: Text,
    opacity: f32,
}

impl ScaleReadout {
    fn new(fonts: &mut Fonts, setting: ScaleSetting, value: f32) -> Self {
        Self {
            setting,
            value,
            text: Self::text(fonts, setting, value),
            opacity: 0.0,
        }
    }

    fn text(fonts: &mut Fonts, setting: ScaleSetting, value: f32) -> Text {
        let label = match setting {
            ScaleSetting::DbRange => format!("RANGE {value:.1} dB"),
            ScaleSetting::Bend => format!("BEND {value:.2}"),
        };
        fonts.layout(&label, 12.0)
    }

    fn draw(
        &mut self,
        fonts: &mut Fonts,
        scene: &mut Scene,
        view: &View,
        scale: &Scale,
        adjusting: Option<ScaleSetting>,
        dt: f32,
    ) {
        let speed = if adjusting == Some(self.setting) {
            8.0
        } else {
            -0.7
        };
        self.opacity = (self.opacity + dt * speed).clamp(0.0, 1.0);
        if self.opacity == 0.0 {
            return;
        }
        let value = match self.setting {
            ScaleSetting::DbRange => scale.negative_db_range,
            ScaleSetting::Bend => scale.bend,
        };
        if self.value != value {
            self.value = value;
            self.text = Self::text(fonts, self.setting, value);
        }
        let width = self.text.width() * 2.0;
        let x = match self.setting {
            ScaleSetting::DbRange => 24.0 + width / 2.0,
            ScaleSetting::Bend => view.viewport.width - 24.0 - width / 2.0,
        };
        self.text.draw(
            scene,
            Affine::translate((x, view.viewport.height - 24.0)) * Affine::scale(2.0),
            Color::new([0.75, 0.7, 0.7, self.opacity]),
        );
    }
}

#[derive(Clone, Copy, PartialEq)]
struct FaceSettings {
    view: View,
    scale: Scale,
    limit: FrameRateLimit,
}

#[derive(Clone, Copy)]
struct NeedleAppearance {
    tip: Point,
    smear: [Point; 2],
    overload: f32,
}

struct Appearance {
    needles: [NeedleAppearance; 2],
    range_opacity: f32,
    bend_opacity: f32,
}

impl Appearance {
    /// Compare with the last frame selected for rasterization so small movements accumulate.
    fn differs(&self, previous: &Self) -> bool {
        self.needles
            .iter()
            .zip(&previous.needles)
            .any(|(current, previous)| {
                current.tip.distance(previous.tip) >= 0.2
                    || current
                        .smear
                        .iter()
                        .zip(previous.smear)
                        .any(|(current, previous)| current.distance(previous) >= 0.2)
                    || (current.overload - previous.overload).abs() >= 1.0 / 1024.0
            })
            || (self.range_opacity - previous.range_opacity).abs() >= 1.0 / 255.0
            || (self.bend_opacity - previous.bend_opacity).abs() >= 1.0 / 255.0
    }
}

struct Readout {
    caption: Text,
    hints: [Text; 2],
}

impl Readout {
    const HINT_OFFSET: f64 = 40.0;

    fn value(fonts: &mut Fonts, text: &str) -> Text {
        fonts.layout(text, 12.0)
    }

    fn new(fonts: &mut Fonts, caption: &str, hints: [&str; 2]) -> Self {
        Self {
            caption: fonts.layout(caption, 8.0),
            hints: hints.map(|hint| fonts.layout(hint, 10.0)),
        }
    }

    fn draw(&self, scene: &mut Scene, transform: Affine, value: &Text) {
        let ink = Color::from_rgb8(80, 72, 72);
        self.caption.draw(scene, transform, ink);
        let baseline = transform * Affine::translate((0.0, 12.0));
        value.draw(scene, baseline, ink);
        for (hint, x) in self
            .hints
            .iter()
            .zip([-Self::HINT_OFFSET, Self::HINT_OFFSET])
        {
            hint.draw(
                scene,
                baseline * Affine::translate((x, 0.0)),
                Color::from_rgb8(100, 90, 90),
            );
        }
    }
}

#[cfg(test)]
mod tests;

impl Needle {
    fn new() -> Self {
        Self {
            overload: 0.0,
            motion: Motion::new(),
        }
    }
}

pub struct Meter {
    processor: Processor,
    pub scale: Scale,
    marks: Vec<Mark>,
    fonts: Fonts,
    preamp_readout: Readout,
    preamp_value: Text,
    frame_readout: Readout,
    frame_values: HashMap<FrameRateLimit, Text>,
    needles: [Needle; 2],
    face: Scene,
    face_settings: Option<FaceSettings>,
    range_readout: ScaleReadout,
    bend_readout: ScaleReadout,
    appearance: Option<Appearance>,
}

impl Meter {
    pub fn new(preamp: PreampDb) -> anyhow::Result<Self> {
        let processor = Processor::new(preamp.gain());
        let mut fonts = Fonts::new()?;
        let preamp_readout = Readout::new(&mut fonts, "PREAMP", ["[Z]", "[X]"]);
        let frame_readout = Readout::new(&mut fonts, "FPS LIMIT", ["[Q]", "[W]"]);
        let frame_values = FrameRateLimit::VARIANTS
            .iter()
            .map(|&limit| (limit, Readout::value(&mut fonts, &limit.to_string())))
            .collect();
        let preamp_value = Readout::value(&mut fonts, &format!("{:.1}dB", preamp.db()));
        let marks = generate_din_scale(&mut fonts);
        let scale = Scale::new();
        let range_readout =
            ScaleReadout::new(&mut fonts, ScaleSetting::DbRange, scale.negative_db_range);
        let bend_readout = ScaleReadout::new(&mut fonts, ScaleSetting::Bend, scale.bend);
        Ok(Self {
            processor,
            scale,
            marks,
            fonts,
            preamp_readout,
            preamp_value,
            frame_readout,
            frame_values,
            needles: std::array::from_fn(|_| Needle::new()),
            face: Scene::new(),
            face_settings: None,
            range_readout,
            bend_readout,
            appearance: None,
        })
    }

    pub fn audio_event(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Config { samplerate } => self.processor.set_samplerate(samplerate),
            AudioEvent::Buffer { buf } => self.processor.consume_buf(&buf),
        }
    }

    pub fn set_preamp(&mut self, preamp: PreampDb) {
        self.processor.preamp = preamp.gain();
        self.preamp_value = Readout::value(&mut self.fonts, &format!("{:.1}dB", preamp.db()));
        self.face_settings = None;
    }

    pub fn reset_scale(&mut self) {
        self.scale = Scale::new();
        self.range_readout.opacity = 1.0;
        self.bend_readout.opacity = 1.0;
    }

    fn draw_face(&mut self, view: &View, limit: FrameRateLimit) {
        let scene = &mut self.face;
        scene.reset();
        let [preamp, frame] = view.layout.readouts();
        self.preamp_readout.draw(
            scene,
            view.transform * Affine::translate(preamp.to_vec2()),
            &self.preamp_value,
        );
        let frame_value = self
            .frame_values
            .get(&limit)
            .expect("every frame limit has a prepared label");
        self.frame_readout.draw(
            scene,
            view.transform * Affine::translate(frame.to_vec2()),
            frame_value,
        );
        for pivot in view.layout.pivots() {
            let meter = view.transform * Affine::translate(pivot.to_vec2());
            draw_scale(scene, meter, &self.marks, &self.scale);
            scene.fill(
                Fill::NonZero,
                meter,
                Color::new([0.0, 0.0, 0.0, 0.25]),
                None,
                &Circle::new((0.0, CENTER_Y * -0.4), 7.0),
            );
        }
    }

    pub fn draw(
        &mut self,
        scene: &mut Scene,
        view: &View,
        limit: FrameRateLimit,
        adjusting: Option<ScaleSetting>,
        dt: f32,
    ) -> Frame<'_> {
        let settings = FaceSettings {
            view: *view,
            scale: self.scale,
            limit,
        };
        let rebuild = self.face_settings != Some(settings);
        if rebuild {
            self.draw_face(view, limit);
            self.face_settings = Some(settings);
        }
        self.range_readout
            .draw(&mut self.fonts, scene, view, &self.scale, adjusting, dt);
        self.bend_readout
            .draw(&mut self.fonts, scene, view, &self.scale, adjusting, dt);
        let levels = self.processor.get_hands_for_instant(Instant::now());
        let pivots = view.layout.pivots();
        let needles = std::array::from_fn(|channel| {
            let needle = &mut self.needles[channel];
            let level = levels[channel];
            let pivot = pivots[channel];
            let meter = view.transform * Affine::translate(pivot.to_vec2());
            let speed = if level >= 1.0 { 25.0 } else { -5.0 };
            needle.overload = (needle.overload + dt * speed).clamp(0.0, 1.0);
            let light = Point::new(0.0, CENTER_Y * -0.4);
            let strength = needle.overload;
            if strength > 0.0 {
                let glow = Gradient::new_two_point_radial(light, 5.0, light, 40.0).with_stops([
                    Color::new([
                        1.0,
                        strength.powi(2) * 0.2,
                        strength.powi(4) * 0.1,
                        strength.powi(2) * 0.2,
                    ]),
                    Color::new([1.0, 0.0, 0.0, 0.0]),
                ]);
                scene.fill(Fill::NonZero, meter, &glow, None, &Circle::new(light, 40.0));
                scene.fill(
                    Fill::NonZero,
                    meter,
                    Color::new([
                        1.0,
                        strength.powi(2) * 0.9,
                        strength.powi(4) * 0.8,
                        strength,
                    ]),
                    None,
                    &Circle::new(light, 5.0),
                );
            }

            let exposure = needle.motion.advance(self.scale.position(level), dt);
            // Vello evaluates sweep angles in [0, TAU); the gauge stays within one turn.
            let angle = Scale::angle(exposure.average as f32) + std::f64::consts::TAU;
            let bounds = [exposure.minimum, exposure.maximum]
                .map(|position| Scale::angle(position as f32) + std::f64::consts::TAU);
            let tip = Point::new(NEEDLE_RADIUS * angle.cos(), NEEDLE_RADIUS * angle.sin());
            let travel = (bounds[1] - bounds[0]) * NEEDLE_RADIUS;
            // Normalize the triangular opacity profile by the tip's swept distance.
            let peak = (2.0 * NEEDLE_WIDTH / travel.max(2.0 * NEEDLE_WIDTH)) as f32;
            let scale = view.transform.as_coeffs()[0].hypot(view.transform.as_coeffs()[1]);
            // Introduce the wings once their sweep exceeds the visible stroke width.
            let transition = ((travel * scale / (NEEDLE_WIDTH * scale).max(1.0) - 1.0) / 2.0)
                .clamp(0.0, 1.0) as f32;
            let blur = transition * transition * (3.0 - 2.0 * transition);
            let clip = Circle::new(Point::ORIGIN, NEEDLE_RADIUS + NEEDLE_WIDTH);
            if peak < 1.0 {
                scene.push_layer(Fill::NonZero, Compose::SrcOver, 1.0, meter, &clip);
            }
            // Merge the physical stroke with the wings before applying their shared opacity.
            scene.stroke(
                &Stroke::new(NEEDLE_WIDTH),
                meter,
                NEEDLE_INK,
                None,
                &Line::new(Point::ORIGIN, tip),
            );
            let smear = bounds.map(|edge| {
                let edge_tip = Point::new(NEEDLE_RADIUS * edge.cos(), NEEDLE_RADIUS * edge.sin());
                if blur > 0.0 && (edge - angle).abs() * NEEDLE_RADIUS * scale > 0.5 {
                    let triangle = Triangle::new(Point::ORIGIN, tip, edge_tip);
                    let gradient = Gradient::new_sweep(Point::ORIGIN, angle as f32, edge as f32)
                        .with_stops([NEEDLE_INK.with_alpha(blur), NEEDLE_INK.with_alpha(0.0)]);
                    scene.fill(Fill::NonZero, meter, &gradient, None, &triangle);
                }
                meter * edge_tip
            });
            if peak < 1.0 {
                let core = peak.sqrt();
                let white = Color::from_rgb8(255, 255, 255);
                let opacity = |coverage| white.with_alpha(1.0 - blur + blur * coverage);
                let mask = Gradient::new_radial(Point::ORIGIN, NEEDLE_RADIUS as f32).with_stops([
                    (0.0, white),
                    (peak, white),
                    (core, opacity(core)),
                    (1.0, opacity(peak)),
                ]);
                scene.push_layer(Fill::NonZero, Compose::DestIn, 1.0, meter, &clip);
                scene.fill(Fill::NonZero, meter, &mask, None, &clip);
                scene.pop_layer();
                scene.pop_layer();
            }
            NeedleAppearance {
                tip: meter * tip,
                smear,
                overload: strength,
            }
        });
        let appearance = Appearance {
            needles,
            range_opacity: self.range_readout.opacity,
            bend_opacity: self.bend_readout.opacity,
        };
        let changed = rebuild
            || self
                .appearance
                .as_ref()
                .is_none_or(|previous| appearance.differs(previous));
        if changed {
            self.appearance = Some(appearance);
        }
        if rebuild {
            Frame::Rebuilt(&self.face)
        } else if changed {
            Frame::Retained(&self.face)
        } else {
            Frame::Unchanged(&self.face)
        }
    }
}
