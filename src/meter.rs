use std::collections::{HashMap, VecDeque};

use strum::VariantArray;
use vello::{
    kurbo::{Affine, Circle, Point, Rect, Size},
    peniko::Fill,
    Scene,
};

use crate::{
    audio::timeline::{AudioClock, AudioInterval, AudioTime},
    config::{Config, PreampDb, ShutterFrames},
    frame_schedule::{FrameRateLimit, FrameTime},
    measurement::Mode,
    processor::Processor,
    render::Frame,
    scales::{generate_scale, Mark, Scale, ScaleSetting},
    text::{Fonts, Text},
    theme::{MeterColors, Theme, ThemeId},
    AudioEvent,
};

const VU_WIDTH: f64 = 320.0;
const VU_HEIGHT: f64 = 220.0;
const CENTER_Y: f64 = 207.0;
const NEEDLE_RADIUS: f64 = 174.0;
const NEEDLE_WIDTH: f64 = 1.0;
const BEND_ROUNDING: f64 = 6.0;
const LAMP_CENTER: Point = Point::new(0.0, CENTER_Y * -0.4);
const LAMP_RADIUS: f64 = 5.0;
const GLOW_RADIUS: f64 = 40.0;

mod motion;
use motion::{Bend, Motion, HINGE_RADIUS, STOP_RADIUS};
pub use motion::{PeakMovement, ShutterWindow};
mod renderer;
pub use renderer::Renderer;

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

const HISTORY_SECONDS: f64 = 2.0;

/// Continuous presentation of recorded audio. Buffering starts only when the
/// source timestamps cannot supply the requested frame; ready future data is
/// sampled directly. A buffered cursor keeps one capture block in reserve.
#[derive(Default)]
enum AudioReadCursor {
    #[default]
    Uninitialized,
    FollowingSource,
    Buffered(AudioClock),
    Waiting(AudioTime),
}

impl AudioReadCursor {
    fn frame(
        &mut self,
        time: FrameTime,
        requested: AudioInterval,
        available: AudioInterval,
    ) -> AudioInterval {
        match *self {
            Self::Uninitialized if available.end() == AudioTime::ZERO => {
                AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO)
            }
            Self::Uninitialized if requested.end() <= available.end() => {
                *self = Self::FollowingSource;
                requested
            }
            Self::Uninitialized => {
                let clock = AudioClock {
                    position: available.start(),
                    at: time.end(),
                };
                *self = Self::Buffered(clock);
                AudioInterval::new(clock.position_at(time.start()), clock.position)
            }
            Self::FollowingSource if requested.end() <= available.end() => requested,
            Self::FollowingSource => {
                *self = Self::Waiting(available.end());
                AudioInterval::new(available.end(), available.end())
            }
            Self::Buffered(mut clock) => {
                let end = clock.position_at(time.end());
                if end > available.end() {
                    *self = Self::Waiting(available.end());
                    return AudioInterval::new(available.end(), available.end());
                }
                // Keep capture/host clock drift or a stalled UI from growing
                // into a replay backlog. Rejoin the newest buffered block.
                if available.start() > end
                    && available.start().seconds_since(end)
                        > available.seconds() + f64::from(time.seconds())
                {
                    clock = AudioClock {
                        position: available.start(),
                        at: time.end(),
                    };
                    *self = Self::Buffered(clock);
                }
                AudioInterval::new(
                    clock.position_at(time.start()),
                    clock.position_at(time.end()),
                )
            }
            Self::Waiting(at) if available.end() <= at => AudioInterval::new(at, at),
            Self::Waiting(at) => {
                let position = available.start().max(at);
                *self = Self::Buffered(AudioClock {
                    position,
                    at: time.end(),
                });
                AudioInterval::new(position, position)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum LampDrive {
    Charging,
    Releasing,
}

impl LampDrive {
    fn speed(self) -> f32 {
        match self {
            Self::Charging => 25.0,
            Self::Releasing => -5.0,
        }
    }
}

struct LampSegment {
    interval: AudioInterval,
    strength: f32,
    drive: LampDrive,
}

impl LampSegment {
    fn value_at(&self, at: AudioTime) -> f32 {
        let seconds = at
            .min(self.interval.end())
            .seconds_since(self.interval.start());
        (self.strength + seconds as f32 * self.drive.speed()).clamp(0.0, 1.0)
    }
}

struct Lamp {
    strength: f32,
    at: AudioTime,
    history: VecDeque<LampSegment>,
}

impl Lamp {
    fn new(at: AudioTime) -> Self {
        Self {
            strength: 0.0,
            at,
            history: VecDeque::with_capacity(2048),
        }
    }

    fn consume(&mut self, level: f32, interval: AudioInterval) {
        assert_eq!(self.at, interval.start(), "lamp drives must be contiguous");
        let segment = LampSegment {
            interval,
            strength: self.strength,
            drive: if level >= 1.0 {
                LampDrive::Charging
            } else {
                LampDrive::Releasing
            },
        };
        self.at = interval.end();
        self.strength = segment.value_at(self.at);
        self.history.push_back(segment);
        let oldest = self.at.shifted(-HISTORY_SECONDS);
        while self
            .history
            .front()
            .is_some_and(|segment| segment.interval.end() <= oldest)
        {
            self.history.pop_front();
        }
    }

    fn value_at(&self, at: AudioTime) -> f32 {
        if at >= self.at {
            return self.strength;
        }
        self.history
            .iter()
            .rev()
            .find(|segment| segment.interval.start() <= at)
            .map_or(0.0, |segment| segment.value_at(at))
    }
}

struct Needle {
    overload: Lamp,
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

    fn advance(
        &mut self,
        fonts: &mut Fonts,
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
    }

    fn draw(&self, scene: &mut Scene, view: &View, colors: &MeterColors) {
        if self.opacity == 0.0 {
            return;
        }
        let width = self.text.width() * 2.0;
        let x = match self.setting {
            ScaleSetting::DbRange => 24.0 + width / 2.0,
            ScaleSetting::Bend => view.viewport.width - 24.0 - width / 2.0,
        };
        self.text.draw(
            scene,
            Affine::translate((
                x,
                view.viewport.height - {
                    let [a, b, ..] = view.transform.as_coeffs();
                    let rows = match self.setting {
                        ScaleSetting::DbRange => 80.0,
                        ScaleSetting::Bend => 28.0,
                    };
                    24.0 + rows * a.hypot(b)
                },
            )) * Affine::scale(2.0),
            colors.scale.with_alpha(self.opacity),
        );
    }
}

#[derive(Clone, Copy, PartialEq)]
struct FaceSettings {
    view: View,
    scale: Scale,
    limit: FrameRateLimit,
    colors: MeterColors,
}

#[derive(Clone, Copy)]
pub struct NeedleAppearance {
    transform: Affine,
    angle: f64,
    bend: Bend,
    edges: [f64; 2],
    peak: f32,
    blur: f32,
    overload: f32,
    width: f64,
}

impl NeedleAppearance {
    fn tip(angle: f64) -> Point {
        Point::new(NEEDLE_RADIUS * angle.cos(), NEEDLE_RADIUS * angle.sin())
    }

    fn hinge(angle: f64) -> Point {
        Point::new(HINGE_RADIUS * angle.cos(), HINGE_RADIUS * angle.sin())
    }

    fn bent_tip(&self, angle: f64) -> Point {
        let hinge = Self::hinge(angle);
        let outer_angle = angle - self.bend.radians();
        hinge
            + (Self::tip(outer_angle) - Point::ORIGIN)
                * ((NEEDLE_RADIUS - HINGE_RADIUS) / NEEDLE_RADIUS)
    }

    fn screen_tip(&self) -> Point {
        self.transform * self.bent_tip(self.angle)
    }

    fn screen_edges(&self) -> [Point; 2] {
        self.edges.map(|edge| self.transform * self.bent_tip(edge))
    }

    fn screen_sweep_extrema(&self) -> impl Iterator<Item = Point> + '_ {
        [Self::hinge(0.0), self.bent_tip(0.0)]
            .into_iter()
            .flat_map(|point| {
                let [a, b, c, d, ..] = self.transform.as_coeffs();
                let x_angle = (c * point.x - a * point.y).atan2(a * point.x + c * point.y);
                let y_angle = (d * point.x - b * point.y).atan2(b * point.x + d * point.y);
                [
                    x_angle,
                    y_angle,
                    x_angle + std::f64::consts::PI,
                    y_angle + std::f64::consts::PI,
                ]
                .into_iter()
                .map(move |angle| (point, angle.rem_euclid(std::f64::consts::TAU)))
            })
            .filter(|(_, angle)| (self.edges[0]..=self.edges[1]).contains(angle))
            .map(|(point, angle)| self.transform * (Affine::rotate(angle) * point))
    }
}

pub struct MeterFrame<'a> {
    pub background: Frame<'a>,
    pub overlay: Overlay<'a>,
}

pub struct Overlay<'a> {
    pub needles: &'a [NeedleAppearance; 2],
    pub colors: MeterColors,
}

struct Appearance {
    needles: [NeedleAppearance; 2],
    range_opacity: f32,
    bend_opacity: f32,
}

impl Appearance {
    /// Compare with the retained pose so small movements accumulate.
    fn needles_differ(&self, previous: &Self) -> bool {
        self.needles
            .iter()
            .zip(&previous.needles)
            .any(|(current, previous)| {
                current.width != previous.width
                    || current.screen_tip().distance(previous.screen_tip()) >= 0.2
                    || current
                        .screen_edges()
                        .iter()
                        .zip(previous.screen_edges())
                        .any(|(current, previous)| current.distance(previous) >= 0.2)
                    || (current.overload - previous.overload).abs() >= 1.0 / 1024.0
            })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum HintOrder {
    BeforeValue,
    AfterValue,
}

struct Readout {
    caption: Option<Text>,
    hints: Vec<Text>,
}

impl Readout {
    const HINT_OFFSET: f64 = 40.0;

    fn value(fonts: &mut Fonts, text: &str) -> Text {
        fonts.layout(text, 12.0)
    }

    fn new(fonts: &mut Fonts, caption: &str, hints: [&str; 2]) -> Self {
        Self {
            caption: Some(fonts.layout(caption, 8.0)),
            hints: hints.map(|hint| fonts.layout(hint, 10.0)).into(),
        }
    }

    fn inline<const N: usize>(fonts: &mut Fonts, hints: [&str; N]) -> Self {
        Self {
            caption: None,
            hints: hints.map(|hint| fonts.layout(hint, 10.0)).into(),
        }
    }

    fn inline_width(&self, value: &Text) -> f64 {
        assert!(
            self.caption.is_none(),
            "inline width belongs to a readout without a caption"
        );
        self.hints
            .iter()
            .map(|hint| hint.width() + 8.0)
            .sum::<f64>()
            + value.width()
    }

    fn hint_offset(value: &Text) -> f64 {
        Self::HINT_OFFSET.max(value.width() / 2.0 + 16.0)
    }

    fn draw(&self, scene: &mut Scene, transform: Affine, value: &Text, colors: &MeterColors) {
        let baseline = transform * Affine::translate((0.0, 12.0));
        match &self.caption {
            Some(caption) => {
                caption.draw(scene, transform, colors.readout);
                value.draw(scene, baseline, colors.readout);
                let offset = Self::hint_offset(value);
                for (hint, x) in self.hints.iter().zip([-offset, offset]) {
                    hint.draw(scene, baseline * Affine::translate((x, 0.0)), colors.hint);
                }
            }
            None => self.draw_inline(scene, transform, value, colors, HintOrder::BeforeValue),
        }
    }

    fn draw_inline(
        &self,
        scene: &mut Scene,
        transform: Affine,
        value: &Text,
        colors: &MeterColors,
        order: HintOrder,
    ) {
        assert!(self.caption.is_none(), "inline readouts have no caption");
        let hints = self.hints.iter().map(|hint| (hint, colors.hint));
        let value = (value, colors.readout);
        let items = (order == HintOrder::AfterValue)
            .then_some(value)
            .into_iter()
            .chain(hints)
            .chain((order == HintOrder::BeforeValue).then_some(value));
        let mut x = 0.0;
        for (text, color) in items {
            text.draw_left(scene, transform * Affine::translate((x, 0.0)), color);
            x += text.width() + 8.0;
        }
    }
}

#[cfg(test)]
mod tests;

impl Needle {
    fn new(mode: Mode, peak_movement: PeakMovement, at: AudioTime) -> Self {
        Self {
            overload: Lamp::new(at),
            motion: Motion::new(mode, peak_movement, at),
        }
    }

    fn consume(&mut self, mode: Mode, scale: &Scale, level: f32, interval: AudioInterval) {
        self.motion.consume(mode.position(scale, level), interval);
        self.overload.consume(level, interval);
    }
}

/// Overrides for embedded gauges. Omitted values follow VU's native styling.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GaugeStyle {
    /// Gauge-local stroke width, shared by the scale and needle.
    pub line_width: Option<f64>,
    pub colors: Option<MeterColors>,
}

pub struct Meter {
    gauge_style: GaugeStyle,
    mode: Mode,
    mode_readout: Readout,
    mode_value: Text,
    peak_movement: PeakMovement,
    peak_movement_readout: Readout,
    peak_movement_value: Text,
    units: Text,
    theme: Theme,
    theme_readout: Readout,
    theme_value: Text,
    shutter_frames: ShutterFrames,
    shutter_readout: Readout,
    shutter_value: Text,
    processor: Processor,
    audio_cursor: AudioReadCursor,
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
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let preamp = config.preamp_db;
        let selected = config.theme;
        let processor = Processor::new(preamp.gain(), config.mode);
        let mut fonts = Fonts::new()?;
        let mode_readout = Readout::inline(&mut fonts, ["[M]"]);
        let mode_value = Readout::value(&mut fonts, config.mode.name());
        let peak_movement_readout = Readout::inline(&mut fonts, ["[N]"]);
        let peak_movement_value = Readout::value(&mut fonts, config.peak_movement.name());
        let units = fonts.layout(config.mode.units(), 14.0);
        let theme_readout = Readout::inline(&mut fonts, ["[A]", "[S]"]);
        let theme_value = Readout::value(&mut fonts, selected.name());
        let shutter_readout = Readout::inline(&mut fonts, ["[C]", "[V]"]);
        let shutter_value = Readout::value(
            &mut fonts,
            &format!("Shutter {:.1}f", config.shutter_frames.frames()),
        );
        let preamp_readout = Readout::new(&mut fonts, "Preamp", ["[Z]", "[X]"]);
        let frame_readout = Readout::new(&mut fonts, "FPS Limit", ["[Q]", "[W]"]);
        let frame_values = FrameRateLimit::VARIANTS
            .iter()
            .map(|&limit| (limit, Readout::value(&mut fonts, &limit.to_string())))
            .collect();
        let preamp_value = Readout::value(&mut fonts, &format!("{:.1}dB", preamp.db()));
        let marks = generate_scale(&mut fonts, config.mode);
        let scale = Scale::new();
        let range_readout =
            ScaleReadout::new(&mut fonts, ScaleSetting::DbRange, scale.negative_db_range);
        let bend_readout = ScaleReadout::new(&mut fonts, ScaleSetting::Bend, scale.bend);
        Ok(Self {
            gauge_style: GaugeStyle::default(),
            mode: config.mode,
            mode_readout,
            mode_value,
            peak_movement: config.peak_movement,
            peak_movement_readout,
            peak_movement_value,
            units,
            theme: Theme::new(selected),
            theme_readout,
            theme_value,
            shutter_frames: config.shutter_frames,
            shutter_readout,
            shutter_value,
            processor,
            audio_cursor: AudioReadCursor::default(),
            scale,
            marks,
            fonts,
            preamp_readout,
            preamp_value,
            frame_readout,
            frame_values,
            needles: std::array::from_fn(|_| {
                Needle::new(config.mode, config.peak_movement, AudioTime::ZERO)
            }),
            face: Scene::new(),
            face_settings: None,
            range_readout,
            bend_readout,
            appearance: None,
        })
    }

    pub fn audio_event(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Config { samplerate } => {
                self.processor.set_samplerate(samplerate);
                self.audio_cursor = AudioReadCursor::default();
                self.needles = std::array::from_fn(|_| {
                    Needle::new(self.mode, self.peak_movement, AudioTime::ZERO)
                });
            }
            AudioEvent::Buffer { buf, captured_at } => self.consume_buf(&buf, captured_at),
        }
    }

    /// Borrowed output audio follows the standalone capture processing path.
    pub fn consume_buf(&mut self, buf: &[[f32; 2]], captured_at: std::time::Instant) {
        for drive in self.processor.consume_buf(buf, captured_at) {
            for (channel, needle) in self.needles.iter_mut().enumerate() {
                needle.consume(
                    self.mode,
                    &self.scale,
                    drive.levels[channel],
                    drive.interval,
                );
            }
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.processor.set_mode(mode);
        self.needles = std::array::from_fn(|_| {
            Needle::new(mode, self.peak_movement, self.processor.audio_time())
        });
        self.mode_value = Readout::value(&mut self.fonts, mode.name());
        self.units = self.fonts.layout(mode.units(), 14.0);
        self.marks = generate_scale(&mut self.fonts, mode);
        self.face_settings = None;
        self.appearance = None;
        self.range_readout.opacity = 0.0;
        self.bend_readout.opacity = 0.0;
    }

    pub fn set_peak_movement(&mut self, movement: PeakMovement) {
        assert!(
            self.mode.uses_peak_movement(),
            "peak movement selection belongs to a peak mode"
        );
        self.peak_movement = movement;
        self.peak_movement_value = Readout::value(&mut self.fonts, movement.name());
        for needle in &mut self.needles {
            needle.motion.set_peak_movement(movement);
        }
        self.face_settings = None;
    }

    pub fn set_theme(&mut self, selected: ThemeId) {
        self.theme.select(selected);
        self.theme_value = Readout::value(&mut self.fonts, selected.name());
        self.face_settings = None;
    }

    pub fn set_shutter(&mut self, shutter: ShutterFrames) {
        self.shutter_frames = shutter;
        self.shutter_value = Readout::value(
            &mut self.fonts,
            &format!("Shutter {:.1}f", shutter.frames()),
        );
        self.face_settings = None;
    }

    pub fn set_preamp(&mut self, preamp: PreampDb) {
        self.processor.preamp = preamp.gain();
        self.preamp_value = Readout::value(&mut self.fonts, &format!("{:.1}dB", preamp.db()));
        self.face_settings = None;
    }

    pub fn repair_needles(&mut self) {
        for needle in &mut self.needles {
            needle.motion.repair();
        }
    }

    pub fn reset_scale(&mut self) {
        self.scale = Scale::new();
        let opacity = if self.mode == Mode::Vu { 0.0 } else { 1.0 };
        self.range_readout.opacity = opacity;
        self.bend_readout.opacity = opacity;
    }

    fn draw_gauge(
        scene: &mut Scene,
        meter: Affine,
        marks: &[Mark],
        scale: &Scale,
        mode: Mode,
        colors: &MeterColors,
        units: &Text,
        line_width: Option<f64>,
    ) {
        crate::scales::draw_scale_styled(scene, meter, marks, scale, mode, colors, line_width);
        units.draw(
            scene,
            meter * Affine::translate((LAMP_CENTER.x, LAMP_CENTER.y - 23.0)),
            colors.scale,
        );
        scene.fill(
            Fill::NonZero,
            meter,
            colors.needle_stop,
            None,
            &Circle::new(motion::stop_center(), STOP_RADIUS),
        );
        scene.fill(
            Fill::NonZero,
            meter,
            colors.lamp_off,
            None,
            &Circle::new(LAMP_CENTER, 7.0),
        );
    }

    fn draw_face(&mut self, view: &View, limit: FrameRateLimit) {
        let scene = &mut self.face;
        scene.reset();
        let colors = &self.theme.colors;
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            colors.background,
            None,
            &Rect::from_origin_size(Point::ORIGIN, view.viewport),
        );
        let [a, b, ..] = view.transform.as_coeffs();
        let theme_scale = a.hypot(b);
        let theme_transform =
            Affine::translate((24.0, view.viewport.height - 24.0)) * Affine::scale(theme_scale);
        self.theme_readout
            .draw(scene, theme_transform, &self.theme_value, colors);
        self.mode_readout.draw(
            scene,
            theme_transform * Affine::translate((0.0, -24.0)),
            &self.mode_value,
            colors,
        );
        if self.mode.uses_peak_movement() {
            self.peak_movement_readout.draw(
                scene,
                theme_transform * Affine::translate((0.0, -48.0)),
                &self.peak_movement_value,
                colors,
            );
        }
        let shutter_width = self.shutter_readout.inline_width(&self.shutter_value) * theme_scale;
        let shutter_transform = Affine::translate((
            view.viewport.width - 24.0 - shutter_width,
            view.viewport.height - 24.0,
        )) * Affine::scale(theme_scale);
        self.shutter_readout.draw_inline(
            scene,
            shutter_transform,
            &self.shutter_value,
            colors,
            HintOrder::AfterValue,
        );
        let [preamp, frame] = view.layout.readouts();
        self.preamp_readout.draw(
            scene,
            view.transform * Affine::translate(preamp.to_vec2()),
            &self.preamp_value,
            colors,
        );
        let frame_value = self
            .frame_values
            .get(&limit)
            .expect("every frame limit has a prepared label");
        self.frame_readout.draw(
            scene,
            view.transform * Affine::translate(frame.to_vec2()),
            frame_value,
            colors,
        );
        for pivot in view.layout.pivots() {
            let meter = view.transform * Affine::translate(pivot.to_vec2());
            Self::draw_gauge(
                scene,
                meter,
                &self.marks,
                &self.scale,
                self.mode,
                colors,
                &self.units,
                None,
            );
        }
    }

    fn expose_needles(
        &mut self,
        transforms: [Affine; 2],
        frame_time: FrameTime,
    ) -> [NeedleAppearance; 2] {
        let requested = AudioInterval::new(
            self.processor
                .position_at(frame_time.start())
                .unwrap_or(AudioTime::ZERO),
            self.processor
                .position_at(frame_time.end())
                .unwrap_or(AudioTime::ZERO),
        );
        let frame =
            self.audio_cursor
                .frame(frame_time, requested, self.processor.available_interval());
        self.appearances_at(transforms, frame)
    }
    fn appearances_at(
        &mut self,
        transforms: [Affine; 2],
        frame: AudioInterval,
    ) -> [NeedleAppearance; 2] {
        std::array::from_fn(|channel| {
            let needle = &mut self.needles[channel];
            let meter = transforms[channel];
            let strength = needle.overload.value_at(frame.end());
            let exposure = needle.motion.expose(frame, self.shutter_frames);
            // Keep angular gradients within one positive turn.
            let angle = Scale::angle(exposure.average as f32) + std::f64::consts::TAU;
            let bounds = [exposure.minimum, exposure.maximum]
                .map(|position| Scale::angle(position as f32) + std::f64::consts::TAU);
            let width = self.gauge_style.line_width.unwrap_or(NEEDLE_WIDTH);
            let travel = (bounds[1] - bounds[0]) * NEEDLE_RADIUS;
            // Normalize the triangular opacity profile by the tip's swept distance.
            let peak = (2.0 * width / travel.max(2.0 * width)) as f32;
            let scale = meter.as_coeffs()[0].hypot(meter.as_coeffs()[1]);
            // Introduce the wings once their sweep exceeds the visible stroke width.
            let transition =
                ((travel * scale / (width * scale).max(1.0) - 1.0) / 2.0).clamp(0.0, 1.0) as f32;
            let blur = transition * transition * (3.0 - 2.0 * transition);
            NeedleAppearance {
                transform: meter,
                angle,
                bend: exposure.bend,
                edges: bounds,
                peak,
                blur,
                overload: strength,
                width,
            }
        })
    }

    pub fn draw(
        &mut self,
        scene: &mut Scene,
        view: &View,
        limit: FrameRateLimit,
        adjusting: Option<ScaleSetting>,
        frame_time: FrameTime,
    ) -> MeterFrame<'_> {
        let dt = frame_time.seconds();
        self.theme.advance(dt);
        let settings = FaceSettings {
            view: *view,
            scale: self.scale,
            limit,
            colors: self.theme.colors,
        };
        let rebuild = self.face_settings != Some(settings);
        if rebuild {
            self.draw_face(view, limit);
            self.face_settings = Some(settings);
        }
        for readout in [&mut self.range_readout, &mut self.bend_readout] {
            readout.advance(&mut self.fonts, &self.scale, adjusting, dt);
            readout.draw(scene, view, &self.theme.colors);
        }
        let needles = self.expose_needles(
            view.layout
                .pivots()
                .map(|pivot| view.transform * Affine::translate(pivot.to_vec2())),
            frame_time,
        );
        let appearance = Appearance {
            needles,
            range_opacity: self.range_readout.opacity,
            bend_opacity: self.bend_readout.opacity,
        };
        let background_changed = rebuild
            || self.appearance.as_ref().is_none_or(|previous| {
                appearance.range_opacity != previous.range_opacity
                    || appearance.bend_opacity != previous.bend_opacity
            });
        let changed = background_changed
            || self
                .appearance
                .as_ref()
                .is_none_or(|previous| appearance.needles_differ(previous));
        if changed {
            self.appearance = Some(appearance);
        }
        let background = if rebuild {
            Frame::Rebuilt(&self.face)
        } else if background_changed {
            Frame::Retained(&self.face)
        } else {
            Frame::Unchanged(&self.face)
        };
        MeterFrame {
            background,
            overlay: Overlay {
                colors: self.theme.colors,
                needles: &self
                    .appearance
                    .as_ref()
                    .expect("the first frame prepares the needles")
                    .needles,
            },
        }
    }
}

/// A host supplies placement and display times; VU retains all meter behavior.
#[derive(Clone, Copy)]
pub struct GaugeFrame {
    pub needles: [NeedleAppearance; 2],
    pub colors: MeterColors,
    pub positions: [f32; 2],
    pub changed: bool,
}
impl Meter {
    pub fn set_gauge_style(&mut self, style: GaugeStyle) {
        assert!(style
            .line_width
            .is_none_or(|width| width.is_finite() && width > 0.0));
        if self.gauge_style != style {
            self.gauge_style = style;
            self.appearance = None;
        }
    }
    pub fn configure(&mut self, config: &Config) {
        if self.mode != config.mode {
            self.set_mode(config.mode);
        }
        if self.peak_movement != config.peak_movement && config.mode.uses_peak_movement() {
            self.set_peak_movement(config.peak_movement);
        }
        if self.processor.preamp != config.preamp_db.gain() {
            self.set_preamp(config.preamp_db);
        }
        if self.shutter_frames != config.shutter_frames {
            self.set_shutter(config.shutter_frames);
        }
        if self.theme.selected() != config.theme {
            self.set_theme(config.theme);
        }
    }
    pub fn draw_gauge_faces(&self, scene: &mut Scene, transforms: [Affine; 2]) {
        for transform in transforms {
            Self::draw_gauge(
                scene,
                transform * Affine::translate((160.0, CENTER_Y)),
                &self.marks,
                &self.scale,
                self.mode,
                &self.gauge_style.colors.unwrap_or(self.theme.colors),
                &self.units,
                self.gauge_style.line_width,
            );
        }
    }
    /// Compare the current recorded pose without adding a display frame to the shutter.
    pub fn gauges_need_frame(&mut self, transforms: [Affine; 2], now: std::time::Instant) -> bool {
        let end = self
            .processor
            .position_at(now)
            .unwrap_or(AudioTime::ZERO)
            .min(self.processor.available_interval().end());
        let needles = self.appearances_at(
            transforms.map(|t| t * Affine::translate((160.0, CENTER_Y))),
            AudioInterval::new(end, end),
        );
        let current = Appearance {
            needles,
            range_opacity: 0.0,
            bend_opacity: 0.0,
        };
        self.theme.animating()
            || self
                .appearance
                .as_ref()
                .is_none_or(|previous| current.needles_differ(previous))
    }
    pub fn update_gauges(&mut self, transforms: [Affine; 2], frame_time: FrameTime) -> GaugeFrame {
        let previous_colors = self.theme.colors;
        self.theme.advance(frame_time.seconds());
        let needles = self.expose_needles(
            transforms.map(|t| t * Affine::translate((160.0, CENTER_Y))),
            frame_time,
        );
        let appearance = Appearance {
            needles,
            range_opacity: 0.0,
            bend_opacity: 0.0,
        };
        let changed = previous_colors != self.theme.colors
            || self
                .appearance
                .as_ref()
                .is_none_or(|previous| appearance.needles_differ(previous));
        if changed {
            self.appearance = Some(appearance);
        }
        let needles = self
            .appearance
            .as_ref()
            .expect("first gauge frame has an appearance")
            .needles;
        GaugeFrame {
            needles,
            colors: self.gauge_style.colors.unwrap_or(self.theme.colors),
            positions: needles.map(|needle| {
                ((needle.angle - std::f64::consts::TAU).to_degrees() as f32
                    + 90.0
                    + crate::scales::MAX_ANGLE)
                    / (2.0 * crate::scales::MAX_ANGLE)
            }),
            changed,
        }
    }
}

/// VU's angular wing profile, independent of the rendered vector geometry.
pub const SWEEP_PROFILE_SHADER: &str = include_str!("meter/profile.wgsl");

/// Shared geometric stroke coverage, sweep wings and radial opacity.
pub const SWEEP_SHADER: &str = include_str!("meter/sweep.wgsl");

impl GaugeFrame {
    pub fn placed(mut self, transforms: [Affine; 2]) -> Self {
        for (needle, transform) in self.needles.iter_mut().zip(transforms) {
            needle.transform = transform * Affine::translate((160.0, CENTER_Y));
        }
        self
    }
}
