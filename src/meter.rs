use std::collections::{HashMap, VecDeque};

use strum::{EnumCount, VariantArray};
use vello::{
    kurbo::{Affine, Circle, Point, Rect, Size, Stroke},
    peniko::{Color, Fill},
    Scene,
};

use crate::{
    audio::timeline::{AudioClock, AudioInterval, AudioTime},
    config::{Config, PreampDb, ShutterFrames, VisualizerTextScale},
    frame_schedule::{FrameRateLimit, FrameTime},
    measurement::Mode,
    processor::Processor,
    render::Frame,
    scales::{generate_scale, Mark, Scale, ScaleSetting},
    text::{Fonts, Text, UiFace},
    theme::{MeterColors, Theme, ThemeId},
    visualizer::{self, Presentation, Visualizer},
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

mod debug;
mod motion;
pub use crate::shutter::ShutterWindow;
pub use motion::PeakMovement;
use motion::{Bend, Motion, HINGE_RADIUS, STOP_RADIUS};
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
    fit_scale: f64,
    viewport: Size,
    ui_scale: f64,
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
            fit_scale: scale,
            viewport,
            ui_scale: 1.0,
        }
    }

    pub fn zoom(&mut self, focus: Point, factor: f64) {
        assert!(factor > 0.0, "zoom must preserve a positive scale");
        let point = self.transform.inverse() * focus;
        self.transform *= Affine::translate(point.to_vec2())
            * Affine::scale(factor)
            * Affine::translate(-point.to_vec2());
    }

    /// Physical pixels per logical UI point, independent of window size.
    pub fn ui_scale(&self) -> f64 {
        self.ui_scale
    }

    pub fn set_ui_scale(&mut self, scale: f64) {
        assert!(
            scale.is_finite() && scale > 0.0,
            "the UI scale must be finite and positive"
        );
        self.ui_scale = scale;
    }

    /// Interactive zoom relative to the fitted gauge size.
    pub fn zoom_factor(&self) -> f64 {
        let [a, b, ..] = self.transform.as_coeffs();
        a.hypot(b) / self.fit_scale
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
            ScaleSetting::DbRange => format!("RANGE {value:>5.1} dB"),
            ScaleSetting::Bend => format!("BEND {value:>6.2}"),
        };
        fonts.layout_gauge(&label, 12.0)
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
    presentation: Presentation,
    debug: bool,
    text_scale: VisualizerTextScale,
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

pub enum Overlay<'a> {
    Gauges(GaugeOverlay<'a>),
    Visualizer(visualizer::Frame<'a>),
}

pub struct GaugeOverlay<'a> {
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
            caption: Some(fonts.layout_ui(caption, 8.0, UiFace::Light)),
            hints: hints
                .map(|hint| fonts.layout_ui(hint, 10.0, UiFace::LightItalic))
                .into(),
        }
    }

    fn inline<const N: usize>(fonts: &mut Fonts, hints: [&str; N]) -> Self {
        Self {
            caption: None,
            hints: hints
                .map(|hint| fonts.layout_ui(hint, 10.0, UiFace::LightItalic))
                .into(),
        }
    }

    fn gauge_value(fonts: &mut Fonts, text: &str) -> Text {
        fonts.layout_gauge(text, 12.0)
    }

    fn gauge_new(fonts: &mut Fonts, caption: &str, hints: [&str; 2]) -> Self {
        Self {
            caption: Some(fonts.layout_gauge(caption, 8.0)),
            hints: hints.map(|hint| fonts.layout_gauge(hint, 10.0)).into(),
        }
    }

    fn gauge_inline<const N: usize>(fonts: &mut Fonts, hints: [&str; N]) -> Self {
        Self {
            caption: None,
            hints: hints.map(|hint| fonts.layout_gauge(hint, 10.0)).into(),
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

    /// Left-aligned caption, value and hints for the full-window HUD.
    fn draw_left_aligned(
        &self,
        scene: &mut Scene,
        transform: Affine,
        value: &Text,
        colors: &MeterColors,
    ) {
        let baseline = transform * Affine::translate((0.0, 12.0));
        match &self.caption {
            Some(caption) => {
                caption.draw_left(scene, transform, colors.readout);
                value.draw_left(scene, baseline, colors.readout);
                let mut x = value.width() + 8.0;
                for hint in &self.hints {
                    hint.draw_left(scene, baseline * Affine::translate((x, 0.0)), colors.hint);
                    x += hint.width() + 8.0;
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

/// How often the retained debug face is refreshed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugRefresh {
    /// Once per completed FFT hop.
    #[default]
    PerHop,
    Hz20,
    Hz10,
    Hz5,
    Hz2,
    Hz1,
}

impl DebugRefresh {
    fn interval(self) -> Option<f32> {
        match self {
            Self::PerHop => None,
            Self::Hz20 => Some(0.05),
            Self::Hz10 => Some(0.1),
            Self::Hz5 => Some(0.2),
            Self::Hz2 => Some(0.5),
            Self::Hz1 => Some(1.0),
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::PerHop => Self::Hz20,
            Self::Hz20 => Self::Hz10,
            Self::Hz10 => Self::Hz5,
            Self::Hz5 => Self::Hz2,
            Self::Hz2 => Self::Hz1,
            Self::Hz1 => Self::PerHop,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PerHop => "Hop",
            Self::Hz20 => "20Hz",
            Self::Hz10 => "10Hz",
            Self::Hz5 => "5Hz",
            Self::Hz2 => "2Hz",
            Self::Hz1 => "1Hz",
        }
    }
}

struct SliderDrag {
    parameter: visualizer::Parameter,
    track: Rect,
}

#[derive(Clone, Copy)]
struct KernelDrag {
    kernel: visualizer::DetectorKernel,
    rect: Rect,
    last: Point,
}

pub struct Meter {
    gauge_style: GaugeStyle,
    presentation: Presentation,
    presentation_readout: Readout,
    presentation_value: Text,
    scene_readout: Readout,
    scene_value: Text,
    scene_name: &'static str,
    debug_readout: Readout,
    debug_value: Text,
    debug: bool,
    debug_refresh: DebugRefresh,
    debug_snapshot: Option<visualizer::DebugSnapshot>,
    debug_selection: Option<visualizer::DebugSelection>,
    tree_target: Option<visualizer::TreeTarget>,
    graph_max: [Option<debug::GraphMax>; visualizer::Property::COUNT],
    debug_pointer: Option<Point>,
    slider_drag: Option<SliderDrag>,
    kernel_drag: Option<KernelDrag>,
    debug_clock: f32,
    debug_analysis_revision: u64,
    text_scale: VisualizerTextScale,
    text_scale_readout: Readout,
    text_scale_value: Text,
    detectors: visualizer::DetectorSettings,
    kick_gap_readout: Readout,
    kick_gap_value: Text,
    drop_gap_readout: Readout,
    drop_gap_value: Text,
    depth_fade: visualizer::DepthFade,
    depth_fade_readout: Readout,
    depth_fade_value: Text,
    near_opacity: visualizer::NearOpacity,
    near_opacity_readout: Readout,
    near_opacity_value: Text,
    diagnosis_text: Option<Text>,
    diagnosis_anchor: Point,
    diagnosis_color: Color,
    diagnosis_revision: u64,
    diagnosis_visible_last_frame: bool,
    visualizer: Visualizer,
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
    gauge_theme_readout: Readout,
    gauge_theme_value: Text,
    shutter_frames: ShutterFrames,
    shutter_readout: Readout,
    shutter_value: Text,
    gauge_shutter_readout: Readout,
    gauge_shutter_value: Text,
    processor: Processor,
    audio_cursor: AudioReadCursor,
    pub scale: Scale,
    marks: Vec<Mark>,
    fonts: Fonts,
    preamp_readout: Readout,
    preamp_value: Text,
    gauge_preamp_readout: Readout,
    gauge_preamp_value: Text,
    frame_readout: Readout,
    frame_values: HashMap<FrameRateLimit, Text>,
    gauge_frame_readout: Readout,
    gauge_frame_values: HashMap<FrameRateLimit, Text>,
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
        let presentation = config.presentation;
        let presentation_readout = Readout::inline(&mut fonts, ["[F]"]);
        let presentation_value = Readout::value(&mut fonts, presentation.name());
        let scene_readout = Readout::inline(&mut fonts, ["FIGURE"]);
        let scene_value = fonts.layout_ui("Cube", 12.0, UiFace::SemiBoldItalic);
        let debug_readout = Readout::inline(&mut fonts, ["[D]", "[B]"]);
        let debug_value = Readout::value(&mut fonts, "Debug Off");
        let text_scale = config.visualizer_text_scale;
        let text_scale_readout = Readout::inline(&mut fonts, ["[-]", "[+]"]);
        let text_scale_value =
            Readout::value(&mut fonts, &format!("Text {:>3}%", text_scale.percent()));
        let detectors = config.detectors;
        let mut visualizer = Visualizer::new();
        visualizer.set_detector_settings(detectors);
        let kick_gap_readout = Readout::new(&mut fonts, "Kick Gap", ["[G]", "[H]"]);
        let kick_gap_value = Readout::value(
            &mut fonts,
            &format!("{:.0}ms", detectors.kick_onset_min_interval * 1000.0),
        );
        let drop_gap_readout = Readout::new(&mut fonts, "Drop Gap", ["[J]", "[K]"]);
        let drop_gap_value =
            Readout::value(&mut fonts, &format!("{:.2}", detectors.drop_gap_threshold));
        let depth_fade = config.depth_fade;
        visualizer.set_depth_fade(depth_fade);
        let depth_fade_readout = Readout::new(&mut fonts, "Depth Fade", ["[O]", "[P]"]);
        let depth_fade_value = Readout::value(&mut fonts, &format!("{:>3}%", depth_fade.percent()));
        let near_opacity = config.near_opacity;
        visualizer.set_near_opacity(near_opacity);
        let near_opacity_readout = Readout::new(&mut fonts, "Near Opacity", ["[U]", "[I]"]);
        let near_opacity_value =
            Readout::value(&mut fonts, &format!("{:>3}%", near_opacity.percent()));
        let scene_name = visualizer.scene_name();
        let mode_readout = Readout::gauge_inline(&mut fonts, ["[M]"]);
        let mode_value = Readout::gauge_value(&mut fonts, config.mode.name());
        let peak_movement_readout = Readout::gauge_inline(&mut fonts, ["[N]"]);
        let peak_movement_value = Readout::gauge_value(&mut fonts, config.peak_movement.name());
        let units = fonts.layout_gauge(config.mode.units(), 14.0);
        let theme_readout = Readout::inline(&mut fonts, ["[A]", "[S]"]);
        let theme_value = Readout::value(&mut fonts, selected.name());
        let gauge_theme_readout = Readout::gauge_inline(&mut fonts, ["[A]", "[S]"]);
        let gauge_theme_value = Readout::gauge_value(&mut fonts, selected.name());
        let shutter_readout = Readout::inline(&mut fonts, ["[C]", "[V]"]);
        let shutter_value = Readout::value(
            &mut fonts,
            &format!("Shutter {:>4.1}f", config.shutter_frames.frames()),
        );
        let gauge_shutter_readout = Readout::gauge_inline(&mut fonts, ["[C]", "[V]"]);
        let gauge_shutter_value = Readout::gauge_value(
            &mut fonts,
            &format!("Shutter {:>4.1}f", config.shutter_frames.frames()),
        );
        let preamp_readout = Readout::new(&mut fonts, "Preamp", ["[Z]", "[X]"]);
        let gauge_preamp_readout = Readout::gauge_new(&mut fonts, "Preamp", ["[Z]", "[X]"]);
        let frame_readout = Readout::new(&mut fonts, "FPS Limit", ["[Q]", "[W]"]);
        let gauge_frame_readout = Readout::gauge_new(&mut fonts, "FPS Limit", ["[Q]", "[W]"]);
        let frame_values = FrameRateLimit::VARIANTS
            .iter()
            .map(|&limit| (limit, Readout::value(&mut fonts, &format!("{limit:>5}"))))
            .collect();
        let gauge_frame_values = FrameRateLimit::VARIANTS
            .iter()
            .map(|&limit| {
                (
                    limit,
                    Readout::gauge_value(&mut fonts, &format!("{limit:>5}")),
                )
            })
            .collect();
        let preamp_value = Readout::value(&mut fonts, &format!("{:.1}dB", preamp.db()));
        let gauge_preamp_value = Readout::gauge_value(&mut fonts, &format!("{:.1}dB", preamp.db()));
        let marks = generate_scale(&mut fonts, config.mode);
        let scale = Scale::new();
        let range_readout =
            ScaleReadout::new(&mut fonts, ScaleSetting::DbRange, scale.negative_db_range);
        let bend_readout = ScaleReadout::new(&mut fonts, ScaleSetting::Bend, scale.bend);
        Ok(Self {
            gauge_style: GaugeStyle::default(),
            presentation,
            presentation_readout,
            presentation_value,
            scene_readout,
            scene_value,
            scene_name,
            debug_readout,
            debug_value,
            debug: false,
            debug_refresh: DebugRefresh::default(),
            debug_snapshot: None,
            debug_selection: None,
            tree_target: None,
            graph_max: [None; visualizer::Property::COUNT],
            debug_pointer: None,
            slider_drag: None,
            kernel_drag: None,
            debug_clock: 0.0,
            debug_analysis_revision: 0,
            text_scale,
            text_scale_readout,
            text_scale_value,
            detectors,
            kick_gap_readout,
            kick_gap_value,
            drop_gap_readout,
            drop_gap_value,
            depth_fade,
            depth_fade_readout,
            depth_fade_value,
            near_opacity,
            near_opacity_readout,
            near_opacity_value,
            diagnosis_text: None,
            diagnosis_anchor: Point::ORIGIN,
            diagnosis_color: Color::new([1.0, 1.0, 1.0, 1.0]),
            diagnosis_revision: 0,
            diagnosis_visible_last_frame: false,
            visualizer,
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
            gauge_theme_readout,
            gauge_theme_value,
            shutter_frames: config.shutter_frames,
            shutter_readout,
            shutter_value,
            gauge_shutter_readout,
            gauge_shutter_value,
            processor,
            audio_cursor: AudioReadCursor::default(),
            scale,
            marks,
            fonts,
            preamp_readout,
            preamp_value,
            gauge_preamp_readout,
            gauge_preamp_value,
            frame_readout,
            frame_values,
            gauge_frame_readout,
            gauge_frame_values,
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
                self.visualizer.set_samplerate(samplerate);
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
        if self.presentation == Presentation::Visualizer {
            self.visualizer.consume_buf(buf, self.processor.preamp);
        }
    }

    pub fn presentation(&self) -> Presentation {
        self.presentation
    }

    pub fn debug(&self) -> bool {
        self.debug
    }

    pub fn set_debug(&mut self, debug: bool) {
        if self.debug == debug {
            return;
        }
        self.debug = debug;
        let label = self.debug_label();
        self.debug_value = Readout::value(&mut self.fonts, &label);
        self.debug_snapshot = None;
        self.debug_selection = None;
        self.tree_target = None;
        self.graph_max = [None; visualizer::Property::COUNT];
        self.clear_drag();
        self.debug_clock = 0.0;
        self.face_settings = None;
    }

    /// Hover state for the analysis panel. The meter owns the layout, so the
    /// host only forwards the physical pointer position.
    pub fn set_pointer(&mut self, pointer: Option<Point>) {
        if self.debug_pointer == pointer {
            return;
        }
        self.debug_pointer = pointer;
        self.update_sliders(pointer);
        self.update_kernels(pointer);
        if self.debug && self.presentation == Presentation::Visualizer {
            self.face_settings = None;
        }
    }

    /// Selects or clears an analysis item under the pointer. Returns true when
    /// the panel consumed the click so the host can skip its drag gesture.
    pub fn click_debug(&mut self, view: &View, position: Point) -> bool {
        if !self.debug
            || self.presentation != Presentation::Visualizer
            || self.debug_snapshot.is_none()
        {
            return false;
        }
        let property = match self.debug_selection {
            Some(visualizer::DebugSelection::Property(property)) => Some(property),
            _ => None,
        };
        let scale = view.ui_scale() * f64::from(self.text_scale.factor());
        if let Some(property) = property {
            let tree = debug::tree_layout(
                view,
                self.text_scale,
                property,
                self.tree_target,
                self.visualizer.mixer(),
            );
            if tree.rect.width() >= debug::TREE_MIN_VISIBLE_WIDTH * scale
                && tree.rect.height() >= 200.0 * scale
                && tree.rect.contains(position)
            {
                if let Some(action) = tree.action_at(position) {
                    self.apply_tree_action(action);
                    return true;
                }
                if let Some(slider) = tree.slider_at(position) {
                    self.begin_slider(slider.parameter, slider.track, position);
                    return true;
                }
                if let Some((kernel, rect, _)) = tree.kernel_at(position) {
                    self.begin_kernel(kernel, rect, position);
                    return true;
                }
                if let Some(row) = tree.row_at(position) {
                    if let Some(contribution) = row.contribution() {
                        let badge = Rect::new(
                            row.rect.x1 - 18.0 * scale,
                            row.rect.y0,
                            row.rect.x1 - 8.0 * scale,
                            row.rect.y1,
                        );
                        if badge.contains(position) {
                            self.visualizer.toggle_contribution_mix(contribution);
                            self.clear_drag();
                            self.face_settings = None;
                            return true;
                        }
                    }
                    self.tree_target = row.target();
                    self.clear_drag();
                    self.face_settings = None;
                }
                return true;
            }
        }
        let detector_focused = self
            .debug_selection
            .is_some_and(|selection| debug::detector_focused(selection, self.tree_target));
        if !detector_focused {
            if let Some(selection) = self.debug_selection {
                if let Some((property, max)) = debug::graph_max_button_at(
                    view,
                    self.text_scale,
                    selection,
                    &self.visualizer,
                    position,
                ) {
                    let slot = &mut self.graph_max[property.index()];
                    *slot = if *slot == Some(max) { None } else { Some(max) };
                    self.clear_drag();
                    self.face_settings = None;
                    return true;
                }
            }
        }
        let layout = debug::Layout::new(view, self.text_scale);
        if !layout.panel.contains(position) {
            return false;
        }
        if let Some((property, button)) = layout.property_button_at(position) {
            match button {
                debug::PropertyButton::Mute => self.visualizer.toggle_property_mute(property),
                debug::PropertyButton::Solo => self.visualizer.toggle_property_solo(property),
            }
            self.clear_drag();
            self.face_settings = None;
            return true;
        }
        let selection = match layout.item_at(position) {
            Some(item) if self.debug_selection == Some(item) => None,
            Some(item) => Some(item),
            None => None,
        };
        if selection != self.debug_selection {
            self.tree_target = None;
            self.clear_drag();
        }
        self.debug_selection = selection;
        self.face_settings = None;
        true
    }

    fn apply_tree_action(&mut self, action: debug::TreeAction) {
        match action {
            debug::TreeAction::ToggleMix(contribution) => {
                self.visualizer.toggle_contribution_mix(contribution);
            }
            debug::TreeAction::AddExtra(point) => {
                let extra = self.visualizer.add_extra(point);
                self.tree_target = Some(visualizer::TreeTarget::Contribution(
                    visualizer::Contribution::Extra(extra),
                ));
            }
            debug::TreeAction::RemoveExtra(extra) => {
                self.visualizer.remove_extra(extra);
                let removed =
                    visualizer::TreeTarget::Contribution(visualizer::Contribution::Extra(extra));
                if self.tree_target == Some(removed) {
                    self.tree_target = Some(visualizer::TreeTarget::MixPoint(extra.point));
                }
            }
            debug::TreeAction::ToggleAdaptive(target) => self.visualizer.toggle_adaptive(target),
        }
        self.clear_drag();
        self.face_settings = None;
    }

    fn begin_slider(&mut self, parameter: visualizer::Parameter, track: Rect, position: Point) {
        self.slider_drag = Some(SliderDrag { parameter, track });
        let value = Self::slider_value(parameter, track, position.x);
        self.set_parameter(parameter, value);
    }

    fn begin_kernel(&mut self, kernel: visualizer::DetectorKernel, rect: Rect, position: Point) {
        self.slider_drag = None;
        self.kernel_drag = Some(KernelDrag {
            kernel,
            rect,
            last: position,
        });
        self.paint_kernel(kernel, rect, position, position);
        self.face_settings = None;
    }

    fn update_kernels(&mut self, pointer: Option<Point>) {
        let (Some(pointer), Some(drag)) = (pointer, self.kernel_drag) else {
            return;
        };
        self.paint_kernel(drag.kernel, drag.rect, drag.last, pointer);
        self.kernel_drag = Some(KernelDrag {
            last: pointer,
            ..drag
        });
    }

    fn paint_kernel(
        &mut self,
        kernel: visualizer::DetectorKernel,
        rect: Rect,
        from: Point,
        to: Point,
    ) {
        let band_at = |x: f64| {
            (((x - rect.x0) / rect.width()).clamp(0.0, 0.999_999) * visualizer::BAND_COUNT as f64)
                as usize
        };
        let value_at = |y: f64| ((rect.y1 - y) / rect.height()).clamp(0.0, 1.0) as f32;
        let from_band = band_at(from.x);
        let to_band = band_at(to.x);
        let from_value = value_at(from.y);
        let to_value = value_at(to.y);
        let (low, high) = if from_band <= to_band {
            (from_band, to_band)
        } else {
            (to_band, from_band)
        };
        for band in low..=high {
            let value = if from_band == to_band {
                to_value
            } else {
                let position =
                    (band as f64 - from_band as f64) / (to_band as f64 - from_band as f64);
                (f64::from(from_value) + (f64::from(to_value) - f64::from(from_value)) * position)
                    as f32
            };
            self.visualizer.set_kernel_band(kernel, band, value);
        }
        self.detectors = self.visualizer.detector_settings();
        self.face_settings = None;
    }

    fn clear_drag(&mut self) {
        self.slider_drag = None;
        self.kernel_drag = None;
    }

    /// Clears the inspector selection without changing the debug panel toggle.
    pub fn clear_debug_selection(&mut self) {
        if self.debug_selection.take().is_some() {
            self.tree_target = None;
            self.clear_drag();
            self.face_settings = None;
        }
    }

    /// Live detector settings, including slider edits, for config sync.
    pub fn detector_settings(&self) -> visualizer::DetectorSettings {
        self.detectors
    }

    fn set_parameter(&mut self, parameter: visualizer::Parameter, value: f32) {
        self.visualizer.set_parameter(parameter, value);
        self.set_detector_settings(self.visualizer.detector_settings());
        self.face_settings = None;
    }

    pub fn end_slider_drag(&mut self) {
        self.clear_drag();
    }

    fn slider_value(parameter: visualizer::Parameter, track: Rect, x: f64) -> f32 {
        let fraction = ((x - track.x0) / track.width().max(1.0)).clamp(0.0, 1.0) as f32;
        parameter.from_fraction(fraction)
    }

    fn update_sliders(&mut self, pointer: Option<Point>) {
        let Some(drag) = &self.slider_drag else {
            return;
        };
        let (parameter, track) = (drag.parameter, drag.track);
        let Some(pointer) = pointer else {
            return;
        };
        let value = Self::slider_value(parameter, track, pointer.x);
        self.set_parameter(parameter, value);
    }

    pub fn cycle_debug_refresh(&mut self) {
        self.debug_refresh = self.debug_refresh.next();
        let label = self.debug_label();
        self.debug_value = Readout::value(&mut self.fonts, &label);
        self.debug_snapshot = None;
        self.debug_clock = 0.0;
        self.face_settings = None;
    }

    fn debug_label(&self) -> String {
        if self.debug {
            format!("Debug {}", self.debug_refresh.name())
        } else {
            "Debug Off".to_owned()
        }
    }

    pub fn set_presentation(&mut self, presentation: Presentation) {
        if self.presentation == presentation {
            return;
        }
        self.presentation = presentation;
        self.presentation_value = Readout::value(&mut self.fonts, presentation.name());
        if presentation == Presentation::Visualizer {
            self.visualizer.reset_audio();
        } else {
            self.appearance = None;
            self.range_readout.opacity = 0.0;
            self.bend_readout.opacity = 0.0;
        }
        self.face_settings = None;
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.processor.set_mode(mode);
        self.needles = std::array::from_fn(|_| {
            Needle::new(mode, self.peak_movement, self.processor.audio_time())
        });
        self.mode_value = Readout::gauge_value(&mut self.fonts, mode.name());
        self.units = self.fonts.layout_gauge(mode.units(), 14.0);
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
        self.peak_movement_value = Readout::gauge_value(&mut self.fonts, movement.name());
        for needle in &mut self.needles {
            needle.motion.set_peak_movement(movement);
        }
        self.face_settings = None;
    }

    pub fn set_theme(&mut self, selected: ThemeId) {
        self.theme.select(selected);
        self.theme_value = Readout::value(&mut self.fonts, selected.name());
        self.gauge_theme_value = Readout::gauge_value(&mut self.fonts, selected.name());
        self.face_settings = None;
    }

    pub fn set_shutter(&mut self, shutter: ShutterFrames) {
        self.shutter_frames = shutter;
        self.shutter_value = Readout::value(
            &mut self.fonts,
            &format!("Shutter {:>4.1}f", shutter.frames()),
        );
        self.gauge_shutter_value = Readout::gauge_value(
            &mut self.fonts,
            &format!("Shutter {:>4.1}f", shutter.frames()),
        );
        self.face_settings = None;
    }

    pub fn set_preamp(&mut self, preamp: PreampDb) {
        self.processor.preamp = preamp.gain();
        let value = format!("{:.1}dB", preamp.db());
        self.preamp_value = Readout::value(&mut self.fonts, &value);
        self.gauge_preamp_value = Readout::gauge_value(&mut self.fonts, &value);
        self.face_settings = None;
    }

    pub fn set_text_scale(&mut self, scale: VisualizerTextScale) {
        if self.text_scale == scale {
            return;
        }
        self.text_scale = scale;
        self.text_scale_value =
            Readout::value(&mut self.fonts, &format!("Text {:>3}%", scale.percent()));
        self.face_settings = None;
    }

    pub fn set_detector_settings(&mut self, settings: visualizer::DetectorSettings) {
        if self.detectors == settings {
            return;
        }
        self.detectors = settings;
        self.visualizer.set_detector_settings(settings);
        self.kick_gap_value = Readout::value(
            &mut self.fonts,
            &format!("{:.0}ms", settings.kick_onset_min_interval * 1000.0),
        );
        self.drop_gap_value = Readout::value(
            &mut self.fonts,
            &format!("{:.2}", settings.drop_gap_threshold),
        );
        self.face_settings = None;
    }

    pub fn set_depth_fade(&mut self, depth_fade: visualizer::DepthFade) {
        if self.depth_fade == depth_fade {
            return;
        }
        self.depth_fade = depth_fade;
        self.visualizer.set_depth_fade(depth_fade);
        self.depth_fade_value =
            Readout::value(&mut self.fonts, &format!("{:>3}%", depth_fade.percent()));
        self.face_settings = None;
    }

    pub fn set_near_opacity(&mut self, near_opacity: visualizer::NearOpacity) {
        if self.near_opacity == near_opacity {
            return;
        }
        self.near_opacity = near_opacity;
        self.visualizer.set_near_opacity(near_opacity);
        self.near_opacity_value =
            Readout::value(&mut self.fonts, &format!("{:>3}%", near_opacity.percent()));
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
        match self.presentation {
            Presentation::Gauges => {
                let [a, b, ..] = view.transform.as_coeffs();
                let theme_scale = a.hypot(b);
                let theme_transform = Affine::translate((24.0, view.viewport.height - 24.0))
                    * Affine::scale(theme_scale);
                let shutter_width =
                    self.shutter_readout.inline_width(&self.shutter_value) * theme_scale;
                let shutter_transform = Affine::translate((
                    view.viewport.width - 24.0 - shutter_width,
                    view.viewport.height - 24.0,
                )) * Affine::scale(theme_scale);
                self.gauge_theme_readout.draw(
                    scene,
                    theme_transform,
                    &self.gauge_theme_value,
                    colors,
                );
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
                self.gauge_shutter_readout.draw_inline(
                    scene,
                    shutter_transform,
                    &self.gauge_shutter_value,
                    colors,
                    HintOrder::AfterValue,
                );
                let [preamp, frame] = view.layout.readouts();
                self.gauge_preamp_readout.draw(
                    scene,
                    view.transform * Affine::translate(preamp.to_vec2()),
                    &self.gauge_preamp_value,
                    colors,
                );
                let frame_value = self
                    .gauge_frame_values
                    .get(&limit)
                    .expect("every frame limit has a prepared label");
                self.gauge_frame_readout.draw(
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
            Presentation::Visualizer => {
                // Visualizer chrome uses fixed logical sizes, scaled by DPI and
                // the user's text-size setting but never by window size.
                let ui = view.ui_scale() * f64::from(self.text_scale.factor());
                let hud_left = Affine::translate((24.0 * ui, view.viewport.height - 24.0 * ui))
                    * Affine::scale(ui);
                let shutter_width = self.shutter_readout.inline_width(&self.shutter_value) * ui;
                let shutter_transform = Affine::translate((
                    view.viewport.width - 24.0 * ui - shutter_width,
                    view.viewport.height - 24.0 * ui,
                )) * Affine::scale(ui);
                let top_left = Affine::translate((24.0 * ui, 32.0 * ui)) * Affine::scale(ui);
                let palette = self.visualizer.palette();
                let hud_colors = MeterColors {
                    readout: palette.neutral[0],
                    hint: palette.neutral[2],
                    ..*colors
                };
                self.theme_readout
                    .draw(scene, hud_left, &self.theme_value, &hud_colors);
                self.presentation_readout.draw_inline(
                    scene,
                    hud_left * Affine::translate((0.0, -24.0)),
                    &self.presentation_value,
                    &hud_colors,
                    HintOrder::BeforeValue,
                );
                self.scene_readout.draw_inline(
                    scene,
                    hud_left * Affine::translate((0.0, -48.0)),
                    &self.scene_value,
                    &hud_colors,
                    HintOrder::BeforeValue,
                );
                self.debug_readout.draw_inline(
                    scene,
                    hud_left * Affine::translate((0.0, -72.0)),
                    &self.debug_value,
                    &hud_colors,
                    HintOrder::BeforeValue,
                );
                self.text_scale_readout.draw_inline(
                    scene,
                    hud_left * Affine::translate((0.0, -96.0)),
                    &self.text_scale_value,
                    &hud_colors,
                    HintOrder::BeforeValue,
                );
                self.shutter_readout.draw_inline(
                    scene,
                    shutter_transform,
                    &self.shutter_value,
                    &hud_colors,
                    HintOrder::AfterValue,
                );
                self.preamp_readout.draw_left_aligned(
                    scene,
                    top_left,
                    &self.preamp_value,
                    &hud_colors,
                );
                let frame_value = self
                    .frame_values
                    .get(&limit)
                    .expect("every frame limit has a prepared label");
                self.frame_readout.draw_left_aligned(
                    scene,
                    top_left * Affine::translate((0.0, 44.0)),
                    frame_value,
                    &hud_colors,
                );
                self.kick_gap_readout.draw_left_aligned(
                    scene,
                    top_left * Affine::translate((0.0, 88.0)),
                    &self.kick_gap_value,
                    &hud_colors,
                );
                self.drop_gap_readout.draw_left_aligned(
                    scene,
                    top_left * Affine::translate((0.0, 132.0)),
                    &self.drop_gap_value,
                    &hud_colors,
                );
                self.depth_fade_readout.draw_left_aligned(
                    scene,
                    top_left * Affine::translate((0.0, 176.0)),
                    &self.depth_fade_value,
                    &hud_colors,
                );
                self.near_opacity_readout.draw_left_aligned(
                    scene,
                    top_left * Affine::translate((0.0, 220.0)),
                    &self.near_opacity_value,
                    &hud_colors,
                );
                if self.debug {
                    if let Some(snapshot) = self.debug_snapshot {
                        let history = self.visualizer.debug_history();
                        let palette = self.visualizer.palette();
                        let visualizer = &self.visualizer;
                        debug::draw(
                            &mut self.fonts,
                            scene,
                            view,
                            &snapshot,
                            history,
                            visualizer,
                            palette,
                            *colors,
                            self.debug_refresh,
                            self.text_scale,
                            self.debug_selection,
                            self.debug_pointer,
                            self.tree_target,
                            &self.graph_max,
                        );
                    }
                }
            }
        }
    }

    /// Audio-time interval covered by one displayed frame.
    fn display_interval(&mut self, frame_time: FrameTime) -> AudioInterval {
        let requested = AudioInterval::new(
            self.processor
                .position_at(frame_time.start())
                .unwrap_or(AudioTime::ZERO),
            self.processor
                .position_at(frame_time.end())
                .unwrap_or(AudioTime::ZERO),
        );
        self.audio_cursor
            .frame(frame_time, requested, self.processor.available_interval())
    }

    fn expose_needles(
        &mut self,
        transforms: [Affine; 2],
        frame_time: FrameTime,
    ) -> [NeedleAppearance; 2] {
        let frame = self.display_interval(frame_time);
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

    fn update_visualizer_overlays(&mut self, dt: f32) {
        let name = self.visualizer.scene_name();
        if name != self.scene_name {
            self.scene_name = name;
            self.scene_value = self.fonts.layout_ui(name, 12.0, UiFace::SemiBoldItalic);
            self.face_settings = None;
        }
        if self.debug {
            self.debug_clock += dt;
            let revision = self.visualizer.analysis_revision();
            let new_analysis = revision != self.debug_analysis_revision;
            let due = self
                .debug_refresh
                .interval()
                .map_or(new_analysis, |interval| self.debug_clock >= interval);
            if self.debug_snapshot.is_none() || due {
                self.debug_clock = 0.0;
                self.debug_analysis_revision = revision;
                self.debug_snapshot = Some(self.visualizer.debug_snapshot());
                self.face_settings = None;
            }
        }
        if let Some(anchor) = self.visualizer.diagnosis_anchor() {
            self.diagnosis_anchor = Point::new(anchor[0] as f64, anchor[1] as f64);
        }
        let diagnosis = self
            .visualizer
            .diagnosis()
            .map(|readout| (readout.revision, readout.text.clone(), readout.color));
        match diagnosis {
            Some((revision, text, color)) if revision != self.diagnosis_revision => {
                self.diagnosis_revision = revision;
                self.diagnosis_text =
                    Some(self.fonts.layout_ui(&text, 11.0, UiFace::SemiBoldItalic));
                self.diagnosis_color = color;
            }
            None if self.diagnosis_text.is_some() => {
                self.diagnosis_text = None;
                self.diagnosis_revision = 0;
            }
            _ => {}
        }
    }

    fn draw_diagnosis(
        scene: &mut Scene,
        text: &Text,
        anchor: Point,
        color: Color,
        colors: MeterColors,
        ui_scale: f64,
    ) {
        let scale = ui_scale;
        let width = 340.0 * scale;
        let height = 13.0 * scale;
        let left = anchor.x;
        let top = anchor.y - height * 0.5;
        let box_rect = Rect::from_origin_size(
            Point::new(left - 2.0 * scale, top - 2.0 * scale),
            Size::new(width + 12.0 * scale, height + 4.0 * scale),
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            colors.background.with_alpha(0.84),
            None,
            &box_rect,
        );
        scene.stroke(
            &Stroke::new(scale),
            Affine::IDENTITY,
            color.with_alpha(0.65),
            None,
            &box_rect,
        );
        scene.stroke(
            &Stroke::new(1.4 * scale),
            Affine::IDENTITY,
            color,
            None,
            &vello::kurbo::Line::new(
                Point::new(left, top - 3.0 * scale),
                Point::new(left, top + height + 3.0 * scale),
            ),
        );
        scene.stroke(
            &Stroke::new(scale),
            Affine::IDENTITY,
            color.with_alpha(0.7),
            None,
            &vello::kurbo::Line::new(
                Point::new(left - 4.0 * scale, anchor.y),
                Point::new(left + 4.0 * scale, anchor.y),
            ),
        );
        let center = top + height * 0.5;
        let baseline = center + text.cap_height() * scale * 0.5;
        text.draw_left(
            scene,
            Affine::translate((left + 6.0 * scale, baseline)) * Affine::scale(scale),
            color,
        );
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
        let tree_open = self.presentation == Presentation::Visualizer
            && self.debug
            && matches!(
                self.debug_selection,
                Some(visualizer::DebugSelection::Property(_))
            );
        if self.presentation == Presentation::Visualizer {
            self.visualizer.set_diagnosis_suppressed(tree_open);
            let frame_interval = self.display_interval(frame_time);
            let palette = visualizer::Palette::from_theme(self.theme.selected());
            self.visualizer.update(
                frame_interval,
                self.shutter_frames,
                dt,
                &palette,
                (view.viewport.width as f32, view.viewport.height as f32),
                view.zoom_factor() as f32,
                view.ui_scale() as f32 * self.text_scale.factor(),
            );
            self.update_visualizer_overlays(dt);
        }
        let settings = FaceSettings {
            view: *view,
            scale: self.scale,
            limit,
            colors: self.theme.colors,
            presentation: self.presentation,
            debug: self.debug,
            text_scale: self.text_scale,
        };
        let rebuild = self.face_settings != Some(settings);
        if rebuild {
            self.draw_face(view, limit);
            self.face_settings = Some(settings);
        }
        match self.presentation {
            Presentation::Gauges => {
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
                    overlay: Overlay::Gauges(GaugeOverlay {
                        colors: self.theme.colors,
                        needles: &self
                            .appearance
                            .as_ref()
                            .expect("the first frame prepares the needles")
                            .needles,
                    }),
                }
            }
            Presentation::Visualizer => {
                if !tree_open {
                    if let Some(text) = &self.diagnosis_text {
                        Self::draw_diagnosis(
                            scene,
                            text,
                            self.diagnosis_anchor,
                            self.diagnosis_color,
                            self.theme.colors,
                            view.ui_scale() * f64::from(self.text_scale.factor()),
                        );
                    }
                }
                let diagnosis_visible = !tree_open && self.diagnosis_text.is_some();
                let foreground_changed = diagnosis_visible || self.diagnosis_visible_last_frame;
                self.diagnosis_visible_last_frame = diagnosis_visible;
                let background = if rebuild {
                    Frame::Rebuilt(&self.face)
                } else if foreground_changed {
                    Frame::Retained(&self.face)
                } else {
                    Frame::Unchanged(&self.face)
                };
                MeterFrame {
                    background,
                    overlay: Overlay::Visualizer(self.visualizer.frame()),
                }
            }
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
        if self.presentation != config.presentation {
            self.set_presentation(config.presentation);
        }
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
        if self.text_scale != config.visualizer_text_scale {
            self.set_text_scale(config.visualizer_text_scale);
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
