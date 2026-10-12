use std::collections::HashMap;

use biquad::{Biquad, Coefficients, DirectForm2Transposed, ToHertz, Q_BUTTERWORTH_F32};
use instant::Instant;
use strum::VariantArray;
use vello::{
    kurbo::{Affine, BezPath, Circle, Line, Point, Size, Stroke},
    peniko::{Color, Fill, Gradient},
    Scene,
};

use crate::{
    config::PreampDb,
    frame_schedule::FrameRateLimit,
    frame_times::FrameTimes,
    processor::Processor,
    scales::{draw_scale, generate_din_scale, Mark, Scale},
    text::{Fonts, Text},
    AudioEvent,
};

const MOTION_FILTER_CUTOFF: f32 = 4.0;
const VU_WIDTH: f64 = 320.0;
const VU_HEIGHT: f64 = 220.0;
const CENTER_Y: f64 = 207.0;

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
pub struct View {
    layout: Layout,
    transform: Affine,
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
        Self { layout, transform }
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
    previous: Point,
    older: Point,
    overload: f32,
    filter: DirectForm2Transposed<f32>,
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

fn motion_coefficients(fps: u32) -> Coefficients<f32> {
    Coefficients::<f32>::from_params(
        biquad::Type::LowPass,
        fps.hz(),
        MOTION_FILTER_CUTOFF.hz(),
        Q_BUTTERWORTH_F32,
    )
    .expect("the motion filter cutoff must be below the frame rate's Nyquist frequency")
}

impl Needle {
    fn new() -> Self {
        let angle = Scale::angle(0.0);
        let tip = Point::new(174.0 * angle.cos(), 174.0 * angle.sin());
        Self {
            previous: tip,
            older: tip,
            overload: 0.0,
            filter: DirectForm2Transposed::<f32>::new(motion_coefficients(60)),
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
    frame_times: FrameTimes,
    last_fps: u32,
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
        Ok(Self {
            processor,
            scale: Scale::new(),
            marks,
            fonts,
            preamp_readout,
            preamp_value,
            frame_readout,
            frame_values,
            needles: std::array::from_fn(|_| Needle::new()),
            frame_times: FrameTimes::new(),
            last_fps: 60,
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
    }

    pub fn draw(&mut self, scene: &mut Scene, view: &View, limit: FrameRateLimit, dt: f32) {
        self.frame_times.update(dt);
        let fps = (1.0 / self.frame_times.average()).max(MOTION_FILTER_CUTOFF * 2.0 + 1.0) as u32;
        if fps != self.last_fps {
            let coefficients = motion_coefficients(fps);
            for needle in &mut self.needles {
                needle.filter.update_coefficients(coefficients);
            }
            self.last_fps = fps;
        }

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
        let levels = self.processor.get_hands_for_instant(Instant::now());
        for ((needle, level), pivot) in self
            .needles
            .iter_mut()
            .zip(levels)
            .zip(view.layout.pivots())
        {
            let meter = view.transform * Affine::translate(pivot.to_vec2());
            draw_scale(scene, meter, &self.marks, &self.scale);

            let speed = if level >= 1.0 { 25.0 } else { -5.0 };
            needle.overload = (needle.overload + dt * speed).clamp(0.0, 1.0);
            let light = Point::new(0.0, CENTER_Y * -0.4);
            scene.fill(
                Fill::NonZero,
                meter,
                Color::new([0.0, 0.0, 0.0, 0.25]),
                None,
                &Circle::new(light, 7.0),
            );
            let strength = needle.overload;
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

            let position = needle.filter.run(self.scale.position(level));
            let angle = Scale::angle(position);
            let tip = Point::new(174.0 * angle.cos(), 174.0 * angle.sin());
            for previous in [needle.older, needle.previous] {
                let mut trail = BezPath::new();
                trail.move_to(Point::ORIGIN);
                trail.line_to(tip);
                trail.line_to(previous);
                trail.close_path();
                scene.fill(
                    Fill::NonZero,
                    meter,
                    Color::from_rgba8(255, 48, 0, 100),
                    None,
                    &trail,
                );
            }
            scene.stroke(
                &Stroke::new(1.0),
                meter,
                Color::from_rgb8(255, 200, 160),
                None,
                &Line::new(Point::ORIGIN, tip),
            );
            needle.older = needle.previous;
            needle.previous = tip;
        }
    }
}
