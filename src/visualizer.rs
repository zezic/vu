//! Spectrum-driven pseudo-3D wireframe presentation.

mod computation;
mod detection;
mod geometry;
mod model;
mod renderer;
mod spectrum;

use computation::{validate_value, MixContext};
pub use computation::{
    BandComponent, BandRouting, ComputeNode, Contribution, ContributionSpec, Curve, Detector,
    EnvelopeTarget, Extra, ExtraRef, FluxSetting, Gain, Gains, Input, InputShape, Mix, MixPoint,
    Mixer, MixerTuning, Parameter, ReferenceTracking, SelectableInput, Source, StyleInput,
    TreeTarget,
};
pub use detection::{
    BandOnsetSettings, DetectorSettings, DropKernel, DropKernels, DropStatus, FluxSettings,
    OnsetFluxStatus, PatternKernel, PatternLayer, QuakeDirection, TemporalPattern, TriggerStatus,
    ABL_AMOUNT_MIN, ABL_GAIN_MAX, ABL_GAIN_MIN, CHOKE_MAX, CHOKE_MIN, DROP_GAP_GAIN_MAX,
    DROP_GAP_GAIN_MIN, DROP_GAP_THRESHOLD_MAX, DROP_GAP_THRESHOLD_MIN, DROP_GAP_WINDOW_MAX,
    DROP_GAP_WINDOW_MIN, DROP_LP_MAX, DROP_LP_MIN, EXCESS_ATTACK_MAX, EXCESS_ATTACK_MIN,
    EXCESS_CAP_MAX, EXCESS_CAP_MIN, EXCESS_FLOOR_MAX, EXCESS_FLOOR_MIN, EXCESS_RELEASE_MAX,
    EXCESS_RELEASE_MIN, FLUX_ATTACK_MAX, FLUX_ATTACK_MIN, FLUX_AVERAGE_MAX, FLUX_AVERAGE_MIN,
    FLUX_INTERVAL_MAX, FLUX_INTERVAL_MIN, FLUX_PRE_GAIN_MAX, FLUX_PRE_GAIN_MIN, FLUX_RELEASE_MAX,
    FLUX_RELEASE_MIN, FLUX_SENSITIVITY_MAX, FLUX_SENSITIVITY_MIN, FLUX_SUPPRESSION_MAX,
    FLUX_SUPPRESSION_MIN, FLUX_THRESHOLD_MIN, ONSET_THRESHOLD_MAX, QUAKE_FACTOR_MAX,
    QUAKE_FACTOR_MIN, SEQUENCE_EVERY_MAX, SEQUENCE_EVERY_MIN, SPRING_DAMPING_MAX,
    SPRING_DAMPING_MIN, SPRING_STIFFNESS_MAX, SPRING_STIFFNESS_MIN,
};
use detection::{OnsetBank, QuakeSequence, Spring};

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    f32::consts::TAU,
};

use serde::{Deserialize, Serialize};
use strum::{EnumCount, VariantArray};
use vello::peniko::Color;

use crate::{
    audio::timeline::{AudioInterval, AudioTime},
    config::ShutterFrames,
    frame_schedule::Adjustment,
    shutter::ShutterWindow,
    theme::{mix, Accent, ThemeId},
};

use geometry::{
    soft_saturate, Camera, Deformation, DepthOpacity, FigureKind, Pose, Projected, Vec2, Vec3,
    Wireframe,
};
pub use spectrum::{RelativeExcessSettings, BAND_COUNT};
use spectrum::{Spectrum, SpectrumAnalyzer};

pub use renderer::Renderer;

const MAX_PENDING_ACTIONS: usize = 64;
const MAX_HISTORY: usize = 17;
const HISTORY_SECONDS: f64 = 2.0;
/// Below this screen-space movement, the swept ribbon is narrower than the
/// line antialiasing, so the analytic line renderer is the better choice.
const MOTION_BLUR_MIN_PIXELS: f32 = 1.0;
/// Inspector traces are sampled independently of the panel refresh rate.
const DEBUG_SAMPLE_INTERVAL: f32 = 1.0 / 60.0;
const DEBUG_HISTORY_SECONDS: f32 = 4.0;
/// The quake spring works in scene units; one unit is this many screen pixels
/// at the reference viewport scale.
const QUAKE_PIXELS: f32 = 24.0;
/// Keep the animation phase bounded without breaking the many non-integer
/// sine/cosine frequencies derived from it. All current multipliers are
/// hundredths, so one hundred turns is a whole period for every one of them.
const PHASE_PERIOD: f32 = TAU * 100.0;

/// The top-level display selected by the presentation hotkey.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presentation {
    Gauges,
    #[default]
    Visualizer,
    Weapon,
}

impl Presentation {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Gauges => Self::Visualizer,
            Self::Visualizer => Self::Weapon,
            Self::Weapon => Self::Gauges,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Gauges => "Gauges",
            Self::Visualizer => "Wireframe",
            Self::Weapon => "Weapon",
        }
    }

    /// Both visualizer presentations consume audio and draw the visualizer HUD.
    pub fn shows_visualizer(self) -> bool {
        matches!(self, Self::Visualizer | Self::Weapon)
    }
}

/// How the weapon wireframe composites over the theme background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelComposite {
    #[default]
    Additive,
    Multiplicative,
}

/// Depth opacity modulation strength: 0 keeps every vertex opaque, 1 makes the
/// farthest vertex fully transparent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct DepthFade(f32);

impl DepthFade {
    const STEP: f32 = 0.05;

    pub fn strength(self) -> f32 {
        self.0
    }

    pub fn percent(self) -> u32 {
        (self.0 * 100.0).round() as u32
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -Self::STEP,
            Adjustment::Increase => Self::STEP,
        };
        Self((self.0 + delta).clamp(0.0, 1.0))
    }
}

impl Default for DepthFade {
    fn default() -> Self {
        Self(0.5)
    }
}

impl TryFrom<f32> for DepthFade {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.is_finite() && (0.0..=1.0).contains(&value),
            "depth_fade must be finite and between 0 and 1"
        );
        Ok(Self(value))
    }
}

impl From<DepthFade> for f32 {
    fn from(value: DepthFade) -> Self {
        value.0
    }
}

/// Minimum opacity at the nearest scene vertex before line-energy brightening.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct NearOpacity(f32);

impl NearOpacity {
    const MIN: f32 = 0.01;
    const MAX: f32 = 1.0;
    const DEFAULT: f32 = 0.6;
    const STEP: f32 = 0.05;
    const LOW_MAX: f32 = 0.1;
    const LOW_STEP: f32 = 0.01;
    const EPSILON: f32 = 1e-4;

    pub fn value(self) -> f32 {
        self.0
    }

    pub fn percent(self) -> u32 {
        (self.0 * 100.0).round() as u32
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease if self.0 <= Self::LOW_MAX + Self::EPSILON => -Self::LOW_STEP,
            Adjustment::Decrease => -Self::STEP,
            Adjustment::Increase if self.0 < Self::LOW_MAX - Self::EPSILON => Self::LOW_STEP,
            Adjustment::Increase => Self::STEP,
        };
        Self((self.0 + delta).clamp(Self::MIN, Self::MAX))
    }
}

impl Default for NearOpacity {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

impl TryFrom<f32> for NearOpacity {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.is_finite() && (Self::MIN..=Self::MAX).contains(&value),
            "near_opacity must be finite and between {MIN} and {MAX}",
            MIN = Self::MIN,
            MAX = Self::MAX
        );
        Ok(Self(value))
    }
}

impl From<NearOpacity> for f32 {
    fn from(value: NearOpacity) -> Self {
        value.0
    }
}

/// Weapon showcase edge crease threshold in degrees. Lower values keep more
/// edge detail; higher values reduce the wireframe to the strongest creases.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct CreaseThreshold(f32);

impl CreaseThreshold {
    pub const MIN_DEGREES: f32 = 10.0;
    pub const MAX_DEGREES: f32 = 80.0;
    pub const MIN: Self = Self(Self::MIN_DEGREES);
    pub const MAX: Self = Self(Self::MAX_DEGREES);
    const STEP_DEGREES: f32 = 5.0;

    pub fn degrees(self) -> f32 {
        self.0
    }

    pub fn whole_degrees(self) -> u32 {
        self.0.round() as u32
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -Self::STEP_DEGREES,
            Adjustment::Increase => Self::STEP_DEGREES,
        };
        Self((self.0 + delta).clamp(Self::MIN_DEGREES, Self::MAX_DEGREES))
    }
}

impl Default for CreaseThreshold {
    fn default() -> Self {
        Self(45.0)
    }
}

impl TryFrom<f32> for CreaseThreshold {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.is_finite() && (Self::MIN_DEGREES..=Self::MAX_DEGREES).contains(&value),
            "crease_threshold must be finite and between {MIN} and {MAX}",
            MIN = Self::MIN_DEGREES,
            MAX = Self::MAX_DEGREES
        );
        Ok(Self(value))
    }
}

impl From<CreaseThreshold> for f32 {
    fn from(value: CreaseThreshold) -> Self {
        value.0
    }
}

/// Base visualizer line-width multiplier. 1.0 is the viewport-derived width.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct LineWidth(f32);

impl LineWidth {
    pub const MIN: f32 = 0.1;
    pub const MAX: f32 = 3.0;
    const STEP: f32 = 0.1;

    pub fn factor(self) -> f32 {
        self.0
    }

    pub fn percent(self) -> u32 {
        (self.0 * 100.0).round() as u32
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -Self::STEP,
            Adjustment::Increase => Self::STEP,
        };
        Self((self.0 + delta).clamp(Self::MIN, Self::MAX))
    }
}

impl Default for LineWidth {
    fn default() -> Self {
        Self(1.0)
    }
}

impl TryFrom<f32> for LineWidth {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.is_finite() && (Self::MIN..=Self::MAX).contains(&value),
            "line_width must be finite and between {MIN} and {MAX}",
            MIN = Self::MIN,
            MAX = Self::MAX
        );
        Ok(Self(value))
    }
}

impl From<LineWidth> for f32 {
    fn from(value: LineWidth) -> Self {
        value.0
    }
}

/// Whether the rigid weapon showcase uses the analytical exposure sweep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelMotion {
    #[default]
    Still,
    Swept,
}

impl ModelMotion {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Still => Self::Swept,
            Self::Swept => Self::Still,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Still => "OFF",
            Self::Swept => "ON",
        }
    }
}

/// Opacity floor reached by edges at high motion-blur speed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct MotionOpacity(f32);

impl MotionOpacity {
    pub const MIN: f32 = 0.0;
    pub const MAX: f32 = 1.0;
    pub const DEFAULT: f32 = 0.35;
    const STEP: f32 = 0.05;

    pub fn value(self) -> f32 {
        self.0
    }

    pub fn percent(self) -> u32 {
        (self.0 * 100.0).round() as u32
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -Self::STEP,
            Adjustment::Increase => Self::STEP,
        };
        Self((self.0 + delta).clamp(Self::MIN, Self::MAX))
    }
}

impl Default for MotionOpacity {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

impl TryFrom<f32> for MotionOpacity {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.is_finite() && (Self::MIN..=Self::MAX).contains(&value),
            "motion_opacity must be finite and between {MIN} and {MAX}",
            MIN = Self::MIN,
            MAX = Self::MAX
        );
        Ok(Self(value))
    }
}

impl From<MotionOpacity> for f32 {
    fn from(value: MotionOpacity) -> Self {
        value.0
    }
}

/// Exponent of the motion-blur retention term. Higher values lower the
/// opacity at middle speeds, making the falloff more aggressive.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct MotionFadePower(f32);

impl MotionFadePower {
    pub const MIN: f32 = 0.1;
    pub const MAX: f32 = 4.0;
    pub const DEFAULT: f32 = 1.0;
    const STEP: f32 = 0.1;

    pub fn power(self) -> f32 {
        self.0
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -Self::STEP,
            Adjustment::Increase => Self::STEP,
        };
        Self((self.0 + delta).clamp(Self::MIN, Self::MAX))
    }
}

impl Default for MotionFadePower {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

impl TryFrom<f32> for MotionFadePower {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.is_finite() && (Self::MIN..=Self::MAX).contains(&value),
            "motion_fade_power must be finite and between {MIN} and {MAX}",
            MIN = Self::MIN,
            MAX = Self::MAX
        );
        Ok(Self(value))
    }
}

impl From<MotionFadePower> for f32 {
    fn from(value: MotionFadePower) -> Self {
        value.0
    }
}

/// Floor and exponent applied to the motion-blur intensity falloff.
#[derive(Clone, Copy)]
struct MotionFade {
    opacity: MotionOpacity,
    power: MotionFadePower,
}

impl MotionFade {
    fn factor(self, distance: f32, width: f32) -> f32 {
        let speed = distance / (width + distance);
        let keep = (1.0 - speed).powf(self.power.power());
        self.opacity.value() + (1.0 - self.opacity.value()) * keep
    }
}

/// Persisted envelope times and pre-envelope shaping for one analysis input.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputTuning {
    pub attack: f32,
    pub release: f32,
    pub shape: InputShape,
}

impl InputTuning {
    fn band(index: usize) -> Self {
        // The factory preset damps this band faster than the linear ramp.
        let (attack, release) = match index {
            7 => (2.0291474, 2.0105636),
            _ => (14.0, 5.0 + 5.0 * index as f32 / (BAND_COUNT - 1) as f32),
        };
        Self {
            attack,
            release,
            shape: InputShape::adaptive(),
        }
    }

    fn power() -> Self {
        Self {
            attack: 18.0,
            release: 4.5,
            shape: InputShape::default(),
        }
    }

    fn bass() -> Self {
        Self {
            attack: 20.0,
            release: 7.0,
            shape: InputShape::default(),
        }
    }

    fn treble() -> Self {
        Self {
            attack: 16.0,
            release: 3.5,
            shape: InputShape::default(),
        }
    }

    fn validate(self, name: &str) -> anyhow::Result<()> {
        validate_value(
            self.attack,
            Parameter::Attack(EnvelopeTarget::Power).range(),
            &format!("{name} attack"),
        )?;
        validate_value(
            self.release,
            Parameter::Release(EnvelopeTarget::Power).range(),
            &format!("{name} release"),
        )?;
        self.shape
            .validate()
            .map_err(|error| error.context(format!("{name} shape")))?;
        Ok(())
    }
}

/// Persisted envelope and shape settings for every analysis input.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputTunings {
    pub bands: [InputTuning; BAND_COUNT],
    pub power: InputTuning,
    pub bass: InputTuning,
    pub treble: InputTuning,
}

impl Default for InputTunings {
    fn default() -> Self {
        Self {
            bands: std::array::from_fn(InputTuning::band),
            power: InputTuning::power(),
            bass: InputTuning::bass(),
            treble: InputTuning::treble(),
        }
    }
}

impl InputTunings {
    fn validate(&self) -> anyhow::Result<()> {
        for (index, input) in self.bands.iter().enumerate() {
            input.validate(&format!("band {index}"))?;
        }
        self.power.validate("power")?;
        self.bass.validate("bass")?;
        self.treble.validate("treble")?;
        Ok(())
    }
}

/// Persisted property mute and solo state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PropertyGatesTuning {
    muted: BTreeSet<Property>,
    soloed: BTreeSet<Property>,
}

impl From<PropertyGates> for PropertyGatesTuning {
    fn from(gates: PropertyGates) -> Self {
        Self {
            muted: Property::VARIANTS
                .iter()
                .copied()
                .filter(|property| gates.is_muted(*property))
                .collect(),
            soloed: Property::VARIANTS
                .iter()
                .copied()
                .filter(|property| gates.is_soloed(*property))
                .collect(),
        }
    }
}

impl From<PropertyGatesTuning> for PropertyGates {
    fn from(gates: PropertyGatesTuning) -> Self {
        let mut result = Self::default();
        for property in gates.muted {
            result.muted[property.index()] = true;
        }
        for property in gates.soloed {
            result.soloed[property.index()] = true;
        }
        result
    }
}

/// Style values pinned from the inspector. Values that are absent keep the
/// per-figure random variation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StyleTuning(BTreeMap<StyleInput, f32>);

impl StyleTuning {
    fn set(&mut self, input: StyleInput, value: f32) {
        self.0.insert(input, value);
    }

    fn apply(&self, style: &mut Style) {
        for (&input, &value) in &self.0 {
            set_style_value(style, input, value);
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        for (&input, &value) in &self.0 {
            validate_value(
                value,
                Parameter::Style(input).range(),
                &format!("{} style", Parameter::Style(input).label()),
            )?;
        }
        Ok(())
    }
}

/// Everything the inspector can tune that survives a restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "VisualizerTuningRaw")]
pub struct VisualizerTuning {
    pub inputs: InputTunings,
    /// Exponential decay rate of the kick envelope.
    pub kick_decay: f32,
    pub gains: Gains,
    pub routing: BandRouting,
    pub mixer: MixerTuning,
    pub gates: PropertyGatesTuning,
    pub style: StyleTuning,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct VisualizerTuningRaw {
    inputs: InputTunings,
    kick_decay: f32,
    gains: Gains,
    routing: BandRouting,
    mixer: MixerTuning,
    gates: PropertyGatesTuning,
    style: StyleTuning,
}

impl TryFrom<VisualizerTuningRaw> for VisualizerTuning {
    type Error = anyhow::Error;

    fn try_from(raw: VisualizerTuningRaw) -> Result<Self, Self::Error> {
        let tuning = Self {
            inputs: raw.inputs,
            kick_decay: raw.kick_decay,
            gains: raw.gains,
            routing: raw.routing,
            mixer: raw.mixer,
            gates: raw.gates,
            style: raw.style,
        };
        tuning.validate()?;
        Ok(tuning)
    }
}

impl VisualizerTuning {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.inputs.validate()?;
        validate_value(self.kick_decay, Parameter::KickDecay.range(), "kick decay")?;
        self.gains.validate()?;
        self.routing.validate()?;
        self.mixer.to_mixer()?;
        self.style.validate()?;
        Ok(())
    }
}

/// One exposure-swept line, in physical pixels, uploaded as a vertex instance.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    /// Start and end in physical pixels: a.xy, b.xy.
    pub endpoints: [f32; 4],
    /// Tint and opacity at the start endpoint; the shader premultiplies before
    /// blending.
    pub start_color: [f32; 4],
    /// Tint and opacity at the end endpoint.
    pub end_color: [f32; 4],
    /// Stroke width, flat-cap flag, padding, padding.
    pub style: [f32; 4],
}

impl LineInstance {
    fn new(start: Vec2, end: Vec2, color: [f32; 4], width: f32) -> Self {
        Self::shaded(start, end, color, color, width)
    }

    fn shaded(
        start: Vec2,
        end: Vec2,
        start_color: [f32; 4],
        end_color: [f32; 4],
        width: f32,
    ) -> Self {
        Self {
            endpoints: [start.x, start.y, end.x, end.y],
            start_color,
            end_color,
            style: [width, 0.0, 0.0, 0.0],
        }
    }

    /// A shaded line with a flat, square end cap instead of the default round
    /// cap. Used for the rigid wireframe model.
    fn flat(
        start: Vec2,
        end: Vec2,
        start_color: [f32; 4],
        end_color: [f32; 4],
        width: f32,
    ) -> Self {
        let mut line = Self::shaded(start, end, start_color, end_color, width);
        line.style[1] = 1.0;
        line
    }
}

/// The selected Gogh palette split into its four neutral and twelve vivid slots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub neutral: [Color; 4],
    pub vivid: [Color; 12],
}

impl Palette {
    pub const VIVID_COUNT: usize = 12;

    pub fn from_theme(theme: ThemeId) -> Self {
        Self {
            neutral: [
                theme.color(Accent::Foreground),
                theme.color(Accent::White),
                theme.color(Accent::BrightBlack),
                theme.color(Accent::Black),
            ],
            vivid: [
                theme.color(Accent::Red),
                theme.color(Accent::Green),
                theme.color(Accent::Yellow),
                theme.color(Accent::Blue),
                theme.color(Accent::Magenta),
                theme.color(Accent::Cyan),
                theme.color(Accent::BrightRed),
                theme.color(Accent::BrightGreen),
                theme.color(Accent::BrightYellow),
                theme.color(Accent::BrightBlue),
                theme.color(Accent::BrightMagenta),
                theme.color(Accent::BrightCyan),
            ],
        }
    }

    fn approach(&mut self, target: Self, dt: f32, rate: f32) {
        let blend = 1.0 - (-rate * dt).exp();
        for (current, target) in self.neutral.iter_mut().zip(target.neutral) {
            *current = mix(*current, target, blend);
        }
        for (current, target) in self.vivid.iter_mut().zip(target.vivid) {
            *current = mix(*current, target, blend);
        }
    }

    pub fn vivid(&self, index: usize) -> Color {
        self.vivid[index % self.vivid.len()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandRole {
    Sub,
    Bass,
    Low,
    Kick,
    Spin,
    Pitch,
    Roll,
    Twist,
    Bulge,
    Ripple,
    Shear,
    Zoom,
    Accent,
    Line,
    Jitter,
    Phase,
}

impl BandRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sub => "SUB",
            Self::Bass => "BASS",
            Self::Low => "LOW",
            Self::Kick => "KICK",
            Self::Spin => "SPIN",
            Self::Pitch => "PITCH",
            Self::Roll => "ROLL",
            Self::Twist => "TWIST",
            Self::Bulge => "BULGE",
            Self::Ripple => "RIPPLE",
            Self::Shear => "SHEAR",
            Self::Zoom => "ZOOM",
            Self::Accent => "ACCENT",
            Self::Line => "LINE",
            Self::Jitter => "JITTER",
            Self::Phase => "PHASE",
        }
    }

    fn for_index(index: usize) -> Self {
        match index {
            0 => Self::Sub,
            1 => Self::Bass,
            2 => Self::Low,
            3 => Self::Kick,
            4 => Self::Spin,
            5 => Self::Pitch,
            6 => Self::Roll,
            7 => Self::Twist,
            8 => Self::Bulge,
            9 => Self::Ripple,
            10 => Self::Shear,
            11 => Self::Zoom,
            12 => Self::Accent,
            13 => Self::Line,
            14 => Self::Jitter,
            _ => Self::Phase,
        }
    }
}

/// One animation value that the analyzer drives. The rendering code keeps no
/// separate registry: this enum owns the name, the description shown in the
/// inspector, and the analysis sources that feed its current value.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    EnumCount,
    VariantArray,
    Serialize,
    Deserialize,
)]
pub enum Property {
    Pulse,
    Spin,
    Pitch,
    Roll,
    Twist,
    Bulge,
    Ripple,
    Shear,
    Zoom,
    Accent,
    Line,
    Jitter,
    Phase,
    Kick,
    Flip,
    Quake,
    Crossfade,
    Morph,
    Entrance,
}

/// One analysis value that a [`Property`] reads. `BassBands` is the low four
/// bands that form the bass envelope. `KickOnset`, `SnareOnset` and `DropOnset`
/// mark the confirmed detectors whose history the inspector overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dependency {
    Band(BandComponent),
    BassBands,
    Power,
    Bass,
    Kick,
    KickOnset,
    SnareOnset,
    DropOnset,
}

impl Property {
    pub fn index(self) -> usize {
        Self::VARIANTS
            .iter()
            .position(|property| *property == self)
            .expect("every property variant is listed in VARIANTS")
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Pulse => "PULSE",
            Self::Spin => "SPIN",
            Self::Pitch => "PITCH",
            Self::Roll => "ROLL",
            Self::Twist => "TWIST",
            Self::Bulge => "BULGE",
            Self::Ripple => "RIPPLE",
            Self::Shear => "SHEAR",
            Self::Zoom => "ZOOM",
            Self::Accent => "ACCENT",
            Self::Line => "LINE",
            Self::Jitter => "JITTER",
            Self::Phase => "PHASE",
            Self::Kick => "KICK",
            Self::Flip => "FLIP",
            Self::Quake => "QUAKE",
            Self::Crossfade => "CROSSFADE",
            Self::Morph => "MORPH",
            Self::Entrance => "ENTRANCE",
        }
    }

    /// One line explaining what the property does to the figure or to the
    /// animation. The inspector shows this when the property is selected.
    pub fn description(self) -> &'static str {
        match self {
            Self::Pulse => {
                "Bass, broadband and kick energy swell the figure and thicken its edges."
            }
            Self::Spin => "Band 04 speeds the continuous yaw rotation; the style axis weights it.",
            Self::Pitch => "Band 05 tilts the camera; the style Y axis scales the tilt.",
            Self::Roll => "Band 06 rolls the camera; the style Z axis scales the roll.",
            Self::Twist => {
                "Band 07 twists vertices around the vertical axis; twist sign reverses it."
            }
            Self::Bulge => "Band 08 inflates the figure radially; deform bias scales the bulge.",
            Self::Ripple => "Band 09 adds a travelling radial wave to every vertex.",
            Self::Shear => "Band 10 slants X by Y; shear sign reverses the slant.",
            Self::Zoom => "Band 11 pulls the camera closer and enlarges the projected figure.",
            Self::Accent => "Band 12 picks the vivid tint of the next spawned figure.",
            Self::Line => "Band 13 raises edge opacity above the near-opacity floor.",
            Self::Jitter => "Band 14 adds per-vertex noise; jitter bias scales it.",
            Self::Phase => {
                "Band 15 speeds the phase clock behind ripple, jitter and tesseract rotation."
            }
            Self::Kick => "A kick-kernel flux peak latches a decaying kick that swells the figure.",
            Self::Flip => {
                "Every Nth snare adds 180 degrees to the pitch baseline through a damped spring."
            }
            Self::Quake => {
                "Kick and drop triggers inject a velocity impulse into a damped spring shake."
            }
            Self::Crossfade => {
                "A drop always retargets the scene; power and kick set the fade rate."
            }
            Self::Morph => {
                "Power sets how quickly style parameters ease toward a hit's new target."
            }
            Self::Entrance => "Power sets how quickly a newly spawned figure fades in.",
        }
    }

    /// Fixed plot range for the property history. Values outside the range are
    /// clamped, which keeps one trace from flattening every other trace.
    pub fn range(self) -> (f32, f32) {
        match self {
            Self::Pulse => (0.3, 1.5),
            Self::Spin => (0.0, 4.5),
            Self::Pitch => (-1.2, 0.8),
            Self::Roll => (-1.2, 1.2),
            Self::Twist => (-2.5, 2.5),
            Self::Bulge => (0.0, 0.6),
            Self::Ripple => (0.0, 1.2),
            Self::Shear => (-0.8, 0.8),
            Self::Zoom => (1.0, 1.2),
            Self::Accent | Self::Crossfade | Self::Entrance => (0.0, 1.0),
            Self::Line => (0.0, 2.0),
            Self::Jitter => (0.0, 0.08),
            Self::Phase => (0.0, 6.0),
            Self::Kick => (0.0, 1.5),
            Self::Flip => (-TAU, TAU),
            Self::Quake => (-1.5, 1.5),
            Self::Morph => (1.0, 4.2),
        }
    }

    /// Value the property contributes while it is muted. Scale, opacity, and
    /// progress multipliers use one; angles, deformations, and rates use zero.
    fn neutral(self) -> f32 {
        match self {
            Self::Pulse | Self::Zoom | Self::Line | Self::Crossfade | Self::Entrance => 1.0,
            _ => 0.0,
        }
    }

    /// The analyzer values read while this property is computed.
    pub fn dependencies(self) -> &'static [Dependency] {
        match self {
            Self::Pulse => &[
                Dependency::Power,
                Dependency::Bass,
                Dependency::BassBands,
                Dependency::Kick,
                Dependency::Band(BandComponent::Zoom),
            ],
            Self::Spin => &[Dependency::Band(BandComponent::Spin), Dependency::Power],
            Self::Pitch => &[Dependency::Band(BandComponent::Pitch), Dependency::Power],
            Self::Roll => &[Dependency::Band(BandComponent::Roll), Dependency::Power],
            Self::Twist => &[Dependency::Band(BandComponent::Twist)],
            Self::Bulge => &[Dependency::Band(BandComponent::Bulge)],
            Self::Ripple => &[Dependency::Band(BandComponent::Ripple)],
            Self::Shear => &[Dependency::Band(BandComponent::Shear)],
            Self::Zoom => &[Dependency::Band(BandComponent::Zoom)],
            Self::Accent => &[Dependency::Band(BandComponent::Accent)],
            Self::Line => &[Dependency::Band(BandComponent::Line)],
            Self::Jitter => &[Dependency::Band(BandComponent::Jitter)],
            Self::Phase => &[Dependency::Band(BandComponent::Phase), Dependency::Power],
            Self::Kick => &[Dependency::KickOnset],
            Self::Flip => &[Dependency::SnareOnset],
            Self::Quake => &[Dependency::KickOnset, Dependency::DropOnset],
            Self::Crossfade => &[Dependency::Power, Dependency::DropOnset, Dependency::Kick],
            Self::Morph => &[Dependency::Power],
            Self::Entrance => &[Dependency::Power],
        }
    }

    /// True when the band's smoothed value reaches this property directly or
    /// through the bass envelope.
    pub fn depends_on_band_value(self, band: usize, routing: &BandRouting) -> bool {
        self.dependencies()
            .iter()
            .any(|dependency| match dependency {
                Dependency::Band(component) => routing[*component] == band,
                Dependency::BassBands => band < 4,
                _ => false,
            })
    }

    pub fn depends_on_band_onset(self, band: usize, detectors: &DetectorSettings) -> bool {
        self.dependencies()
            .iter()
            .any(|dependency| match dependency {
                Dependency::KickOnset => detectors.kick_contains(band),
                Dependency::SnareOnset => detectors.snare_contains(band),
                // A drop listens to onsets from every band.
                Dependency::DropOnset => true,
                _ => false,
            })
    }

    pub fn depends_on_band(
        self,
        band: usize,
        routing: &BandRouting,
        detectors: &DetectorSettings,
    ) -> bool {
        self.depends_on_band_value(band, routing) || self.depends_on_band_onset(band, detectors)
    }

    /// Multipliers attached to one component in this property's tree. The mix
    /// point definitions are the only source of these gains, so a tree term and
    /// the inspector caption cannot disagree about which multiplier it uses.
    pub fn component_gains(self, input: Input) -> Vec<Gain> {
        let mut gains = Vec::new();
        for point in self.mix_points() {
            for spec in point.contributions() {
                let matches = spec.input() == Some(input)
                    || spec.style_scale.map(Input::Style) == Some(input);
                if let (true, Some(gain)) = (matches, spec.gain) {
                    if !gains.contains(&gain) {
                        gains.push(gain);
                    }
                }
            }
        }
        gains
    }
}

/// Mute and solo state for the properties in the inspector list.
#[derive(Clone, Copy, Debug, Default)]
struct PropertyGates {
    muted: [bool; Property::COUNT],
    soloed: [bool; Property::COUNT],
}

impl PropertyGates {
    fn is_muted(self, property: Property) -> bool {
        self.muted[property.index()]
    }

    fn is_soloed(self, property: Property) -> bool {
        self.soloed[property.index()]
    }

    fn any_solo(self) -> bool {
        self.soloed.iter().any(|&solo| solo)
    }

    /// A property plays unless it is muted, and soloing any property silences
    /// every property that is not soloed.
    fn plays(self, property: Property) -> bool {
        !self.is_muted(property) && (!self.any_solo() || self.is_soloed(property))
    }

    /// The live value while the property plays, its neutral value otherwise.
    fn drive(self, property: Property, value: f32) -> f32 {
        if self.plays(property) {
            value
        } else {
            property.neutral()
        }
    }

    fn toggle_mute(&mut self, property: Property) {
        let muted = &mut self.muted[property.index()];
        *muted = !*muted;
    }

    fn toggle_solo(&mut self, property: Property) {
        let soloed = &mut self.soloed[property.index()];
        *soloed = !*soloed;
    }
}

/// Analysis values that one input reads. Extra mix inputs reuse this mapping so
/// the band dependency panel and the graph inspector follow them.
fn input_dependencies(input: Input) -> Vec<Dependency> {
    match input {
        Input::Band(component) => vec![Dependency::Band(component)],
        Input::BassBands => vec![Dependency::BassBands],
        Input::Power => vec![Dependency::Power],
        // The bass envelope is read from the low bands.
        Input::Bass => vec![Dependency::Bass, Dependency::BassBands],
        Input::Kick => vec![Dependency::Kick],
        Input::KickOnset => vec![Dependency::KickOnset],
        Input::SnareOnset => vec![Dependency::SnareOnset],
        Input::DropOnset => vec![Dependency::DropOnset],
        Input::PowerLevel
        | Input::BassLevel
        | Input::PhaseClock
        | Input::Style(_)
        | Input::Property(_)
        | Input::Constant(_) => Vec::new(),
    }
}

/// A band, a property, or the per-band onset analyzer selected in the
/// analysis panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugSelection {
    Band(usize),
    Property(Property),
    Flux,
}

impl DebugSelection {
    pub fn color(self, palette: Palette) -> Color {
        match self {
            Self::Band(index) => palette.vivid(index),
            Self::Property(property) => palette.vivid(property.index()),
            Self::Flux => palette.vivid(9),
        }
    }
}

/// One sampled frame of every analysis value shown by the inspector. Samples
/// are recorded at display rate, independent of the panel's refresh rate.
#[derive(Clone, Copy, Debug)]
pub struct DebugSample {
    /// Display time in seconds.
    pub at: f32,
    pub bands: [f32; BAND_COUNT],
    pub raw_bands: [f32; BAND_COUNT],
    /// Relative spectral excess, continuous onset flow, and its scaling terms.
    pub band_excess: [f32; BAND_COUNT],
    pub band_onset: [f32; BAND_COUNT],
    pub band_abdr: [f32; BAND_COUNT],
    pub band_abl: [f32; BAND_COUNT],
    pub band_drp: [f32; BAND_COUNT],
    pub power: f32,
    pub raw_power: f32,
    pub bass: f32,
    pub raw_bass: f32,
    pub kick: f32,
    pub kick_onset: TriggerStatus,
    pub snare_onset: TriggerStatus,
    pub drop_onset: TriggerStatus,
    pub drop_status: DropStatus,
    pub kick_flux: OnsetFluxStatus,
    pub snare_flux: OnsetFluxStatus,
    pub properties: [f32; Property::COUNT],
}

impl DebugSample {
    pub fn property(self, property: Property) -> f32 {
        self.properties[property.index()]
    }
}

#[derive(Default)]
pub struct DebugHistory {
    samples: VecDeque<DebugSample>,
}

impl DebugHistory {
    pub fn samples(&self) -> impl DoubleEndedIterator<Item = &DebugSample> + '_ {
        self.samples.iter()
    }

    pub fn latest(&self) -> Option<&DebugSample> {
        self.samples.back()
    }

    /// Time covered by the newest sample and the oldest sample still retained.
    pub fn span(&self) -> Option<(f32, f32)> {
        Some((self.samples.front()?.at, self.samples.back()?.at))
    }

    fn clear(&mut self) {
        self.samples.clear();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BandDebug {
    pub value: f32,
    pub raw: f32,
    pub excess: f32,
    pub onset: f32,
    pub abdr: f32,
    pub abl: f32,
    pub drp: f32,
    pub low_hz: f32,
    pub high_hz: f32,
    pub role: BandRole,
}

#[derive(Clone, Copy, Debug)]
pub struct EnvelopeDebug {
    pub smooth: f32,
    pub raw: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct StyleDebug {
    pub axis: [f32; 3],
    pub twist_sign: f32,
    pub shear_sign: f32,
    pub deform_bias: f32,
    pub zoom_bias: f32,
    pub phase_rate: f32,
    pub jitter_bias: f32,
}

#[derive(Clone, Debug)]
pub struct DebugSnapshot {
    pub bands: [BandDebug; BAND_COUNT],
    pub properties: [f32; Property::COUNT],
    pub power: EnvelopeDebug,
    pub bass: EnvelopeDebug,
    pub treble: EnvelopeDebug,
    pub kick: f32,
    pub kick_onset: TriggerStatus,
    pub snare_onset: TriggerStatus,
    pub drop_onset: TriggerStatus,
    pub drop_status: DropStatus,
    pub kick_flux: OnsetFluxStatus,
    pub snare_flux: OnsetFluxStatus,
    pub kick_hits: u32,
    pub snare_hits: u32,
    pub drop_hits: u32,
    pub scene: String,
    pub outgoing: Option<String>,
    pub transition: f32,
    pub style: StyleDebug,
    pub phase: f32,
    pub entrance: f32,
    pub lines: usize,
    pub ribbons: usize,
}

/// One gradient section of a three-pose motion blur. Ordinary sweeps use a swept
/// quad between poses; crossing sweeps use per-vertex triangle instances with an
/// independent opacity at each corner.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RibbonInstance {
    /// First two triangle vertices: a.xy, b.xy.
    pub start: [f32; 4],
    /// Third triangle vertex duplicated: c.xy, c.xy.
    pub end: [f32; 4],
    /// Tint and opacity at the first vertex.
    pub start_color: [f32; 4],
    /// Tint and opacity at the second vertex.
    pub end_color: [f32; 4],
    /// Tint and opacity at the third vertex.
    pub third_color: [f32; 4],
    /// Wing-mask flag, padding, padding, padding.
    pub style: [f32; 4],
}

impl RibbonInstance {
    /// A single alpha-interpolated triangle. The three vertices carry
    /// independent colors.
    fn triangle(points: [Vec2; 3], colors: [[f32; 4]; 3]) -> Self {
        Self {
            start: [points[0].x, points[0].y, points[1].x, points[1].y],
            end: [points[2].x, points[2].y, points[2].x, points[2].y],
            start_color: colors[0],
            end_color: colors[1],
            third_color: colors[2],
            style: [0.0; 4],
        }
    }

    /// A crossing wing triangle. The first vertex is the outer A/B endpoint,
    /// the second is the body-edge endpoint, and the third is the dual X point.
    /// The shader masks the outer-to-X edge to zero.
    fn wing_triangle(points: [Vec2; 3], colors: [[f32; 4]; 3]) -> Self {
        let mut ribbon = Self::triangle(points, colors);
        ribbon.style[0] = 1.0;
        ribbon
    }
}

#[derive(Clone, Debug)]
pub struct DiagnosisReadout {
    pub revision: u64,
    pub text: String,
    pub color: Color,
}

#[derive(Clone, Copy, Debug)]
enum DiagnosisMode {
    Position,
    World,
    Velocity,
    Acceleration,
}

impl DiagnosisMode {
    fn next(self) -> Self {
        match self {
            Self::Position => Self::World,
            Self::World => Self::Velocity,
            Self::Velocity => Self::Acceleration,
            Self::Acceleration => Self::Position,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Position => 0,
            Self::World => 1,
            Self::Velocity => 2,
            Self::Acceleration => 3,
        }
    }

    fn random(rng: &mut Rng) -> Self {
        match rng.range(4) {
            0 => Self::Position,
            1 => Self::World,
            2 => Self::Velocity,
            _ => Self::Acceleration,
        }
    }

    fn format(self, data: VertexDiagnosis) -> String {
        match self {
            Self::Position => format!("POS X{:7.1} Y{:7.1}", data.screen.x, data.screen.y),
            Self::World => format!(
                "XYZ {:+.2} {:+.2} {:+.2}",
                data.world.x, data.world.y, data.world.z
            ),
            Self::Velocity => format!(
                "VEL {:6.1}px/s  DX{:+.1} DY{:+.1}",
                data.velocity.length(),
                data.velocity.x,
                data.velocity.y
            ),
            Self::Acceleration => format!("ACC {:6.1}px/s2", data.acceleration.length()),
        }
    }
}

struct Diagnosis {
    active: bool,
    /// The inspector tree owns the left side while a property is selected.
    suppressed: bool,
    vertex: usize,
    mode: DiagnosisMode,
    next_in: f32,
    remaining: f32,
    mode_timer: f32,
    refresh_timer: f32,
    anchor: Vec2,
    readout: Option<DiagnosisReadout>,
    revision: u64,
    scene: String,
}

impl Diagnosis {
    fn new() -> Self {
        Self {
            active: false,
            suppressed: false,
            vertex: 0,
            mode: DiagnosisMode::Position,
            next_in: 2.5,
            remaining: 0.0,
            mode_timer: 0.0,
            refresh_timer: 0.0,
            anchor: Vec2::new(0.0, 0.0),
            readout: None,
            revision: 0,
            scene: String::new(),
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.readout = None;
        self.next_in = 2.0;
        self.revision += 1;
    }
}

/// The animated layers for one displayed visualizer frame.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub lines: &'a [LineInstance],
    pub model_lines: &'a [LineInstance],
    pub ribbons: &'a [RibbonInstance],
    pub model_ribbons: &'a [RibbonInstance],
    pub model_composite: ModelComposite,
}

/// Lazily loaded weapon model. The loaded copy is retained so switching back to
/// the showcase does not re-read the file.
enum Showcase {
    Unloaded,
    Ready(Box<model::ModelSet>),
    Failed(String),
}

pub struct Visualizer {
    analyzer: SpectrumAnalyzer,
    bands: [Envelope; BAND_COUNT],
    power: Envelope,
    bass: Envelope,
    treble: Envelope,
    detectors: DetectorSettings,
    onsets: OnsetBank,
    pending_actions: VecDeque<Action>,
    rng: Rng,
    scene: Scene,
    presentation: Presentation,
    showcase: Showcase,
    model_composite: ModelComposite,
    model_lines: Vec<LineInstance>,
    model_ribbons: Vec<RibbonInstance>,
    line_width: LineWidth,
    model_motion: ModelMotion,
    motion_opacity: MotionOpacity,
    motion_fade_power: MotionFadePower,
    frames: ShutterWindow,
    kick: f32,
    kick_decay: f32,
    flip: Spring,
    flip_target: f32,
    flip_count: u32,
    quake_x: Spring,
    quake_y: Spring,
    kick_sequence: QuakeSequence,
    gains: Gains,
    routing: BandRouting,
    mixer: Mixer,
    property_gates: PropertyGates,
    band_shapes: [InputShape; BAND_COUNT],
    power_shape: InputShape,
    bass_shape: InputShape,
    treble_shape: InputShape,
    style_tuning: StyleTuning,
    time: f32,
    palette: Palette,
    depth_fade: DepthFade,
    near_opacity: NearOpacity,
    crease_threshold: CreaseThreshold,
    last_spectrum: Spectrum,
    analysis_revision: u64,
    kick_hits: u32,
    snare_hits: u32,
    drop_hits: u32,
    diagnosis: Diagnosis,
    debug_history: DebugHistory,
    lines: Vec<LineInstance>,
    ribbons: Vec<RibbonInstance>,
}

impl Visualizer {
    pub fn new() -> Self {
        let tuning = VisualizerTuning::default();
        let inputs = tuning.inputs;
        let mut rng = Rng::new(0x005e_ed0f_7e5e_1e55u64);
        let scene = Scene::new(&mut rng);
        let mut visualizer = Self {
            analyzer: SpectrumAnalyzer::new(48_000),
            bands: inputs
                .bands
                .map(|input| Envelope::new(input.attack, input.release)),
            power: Envelope::new(inputs.power.attack, inputs.power.release),
            bass: Envelope::new(inputs.bass.attack, inputs.bass.release),
            treble: Envelope::new(inputs.treble.attack, inputs.treble.release),
            detectors: DetectorSettings::default(),
            onsets: OnsetBank::new(),
            pending_actions: VecDeque::with_capacity(MAX_PENDING_ACTIONS),
            rng,
            scene,
            presentation: Presentation::Visualizer,
            showcase: Showcase::Unloaded,
            model_composite: ModelComposite::default(),
            model_lines: Vec::new(),
            model_ribbons: Vec::new(),
            line_width: LineWidth::default(),
            model_motion: ModelMotion::default(),
            motion_opacity: MotionOpacity::default(),
            motion_fade_power: MotionFadePower::default(),
            frames: ShutterWindow::default(),
            kick: 0.0,
            kick_decay: tuning.kick_decay,
            flip: Spring::default(),
            flip_target: 0.0,
            flip_count: 0,
            quake_x: Spring::default(),
            quake_y: Spring::default(),
            kick_sequence: QuakeSequence::default(),
            gains: tuning.gains,
            routing: tuning.routing,
            mixer: tuning
                .mixer
                .to_mixer()
                .expect("the default visualizer tuning is valid"),
            property_gates: tuning.gates.into(),
            band_shapes: inputs.bands.map(|input| input.shape),
            power_shape: inputs.power.shape,
            bass_shape: inputs.bass.shape,
            treble_shape: inputs.treble.shape,
            style_tuning: tuning.style,
            time: 0.0,
            palette: Palette::from_theme(ThemeId::default()),
            depth_fade: DepthFade::default(),
            near_opacity: NearOpacity::default(),
            crease_threshold: CreaseThreshold::default(),
            last_spectrum: Spectrum::default(),
            analysis_revision: 0,
            kick_hits: 0,
            snare_hits: 0,
            drop_hits: 0,
            diagnosis: Diagnosis::new(),
            debug_history: DebugHistory::default(),
            lines: Vec::with_capacity(2048),
            ribbons: Vec::with_capacity(2048),
        };
        visualizer
            .analyzer
            .set_relative_excess(visualizer.detectors.excess);
        visualizer.apply_style_tuning();
        visualizer
    }

    /// Switches the visualizer content with the presentation. The weapon model
    /// is loaded the first time the showcase becomes visible.
    pub fn set_presentation(&mut self, presentation: Presentation) {
        assert!(
            presentation.shows_visualizer(),
            "the visualizer only handles its own presentations"
        );
        if self.presentation == presentation {
            return;
        }
        self.presentation = presentation;
        match presentation {
            Presentation::Weapon => self.enter_showcase(),
            Presentation::Visualizer if self.scene.current.is_showcase() => {
                let accent = self.accent();
                self.scene
                    .leave_showcase(&mut self.rng, accent, &self.style_tuning);
            }
            _ => {}
        }
    }

    fn enter_showcase(&mut self) {
        let accent = self.accent();
        if let Showcase::Ready(models) = &self.showcase {
            self.scene.enter_showcase(
                models.current(),
                self.crease_threshold,
                self.model_motion,
                accent,
                &self.style_tuning,
            );
            return;
        }
        if let Showcase::Failed(error) = &self.showcase {
            log::error!("retrying the weapon showcase after: {error}");
        }
        match model::ModelSet::load() {
            Ok(models) => {
                let (_, total) = models.position();
                log::info!("weapon showcase: {total} models");
                self.scene.enter_showcase(
                    models.current(),
                    self.crease_threshold,
                    self.model_motion,
                    accent,
                    &self.style_tuning,
                );
                self.showcase = Showcase::Ready(Box::new(models));
            }
            Err(error) => {
                log::error!("could not load the weapon showcase: {error:#}");
                self.showcase = Showcase::Failed(format!("{error:#}"));
            }
        }
    }

    /// Steps to the next showcase model. Drops call this while the showcase is
    /// active; the hotkey calls it from outside a frame.
    fn advance_model(&mut self, accent: f32) {
        let Showcase::Ready(models) = &mut self.showcase else {
            return;
        };
        if !models.advance() {
            return;
        }
        self.scene.switch_showcase(
            models.current(),
            self.crease_threshold,
            self.model_motion,
            accent,
        );
    }

    pub fn next_model(&mut self) {
        if self.presentation == Presentation::Weapon {
            let accent = self.accent();
            self.advance_model(accent);
        }
    }

    pub fn model_position(&self) -> Option<(usize, usize)> {
        match &self.showcase {
            Showcase::Ready(models) => Some(models.position()),
            _ => None,
        }
    }

    /// Load persisted inspector tuning. The value is validated before it
    /// reaches the visualizer.
    pub fn set_tuning(&mut self, tuning: VisualizerTuning) {
        tuning
            .validate()
            .expect("visualizer tuning is validated before it reaches the visualizer");
        let inputs = tuning.inputs;
        for (envelope, input) in self.bands.iter_mut().zip(inputs.bands) {
            envelope.attack = input.attack;
            envelope.release = input.release;
        }
        self.power.attack = inputs.power.attack;
        self.power.release = inputs.power.release;
        self.bass.attack = inputs.bass.attack;
        self.bass.release = inputs.bass.release;
        self.treble.attack = inputs.treble.attack;
        self.treble.release = inputs.treble.release;
        self.band_shapes = inputs.bands.map(|input| input.shape);
        self.power_shape = inputs.power.shape;
        self.bass_shape = inputs.bass.shape;
        self.treble_shape = inputs.treble.shape;
        self.kick_decay = tuning.kick_decay;
        self.gains = tuning.gains;
        self.routing = tuning.routing;
        self.mixer = tuning
            .mixer
            .to_mixer()
            .expect("validated mixer tuning builds a mixer");
        self.property_gates = tuning.gates.into();
        self.style_tuning = tuning.style;
        self.apply_style_tuning();
    }

    /// The live inspector tuning, for config sync.
    pub fn tuning(&self) -> VisualizerTuning {
        VisualizerTuning {
            inputs: InputTunings {
                bands: std::array::from_fn(|index| InputTuning {
                    attack: self.bands[index].attack,
                    release: self.bands[index].release,
                    shape: self.band_shapes[index].persisted(),
                }),
                power: InputTuning {
                    attack: self.power.attack,
                    release: self.power.release,
                    shape: self.power_shape.persisted(),
                },
                bass: InputTuning {
                    attack: self.bass.attack,
                    release: self.bass.release,
                    shape: self.bass_shape.persisted(),
                },
                treble: InputTuning {
                    attack: self.treble.attack,
                    release: self.treble.release,
                    shape: self.treble_shape.persisted(),
                },
            },
            kick_decay: self.kick_decay,
            gains: self.gains,
            routing: self.routing,
            mixer: self.mixer.tuning(),
            gates: self.property_gates.into(),
            style: self.style_tuning.clone(),
        }
    }

    fn apply_style_tuning(&mut self) {
        self.style_tuning.apply(&mut self.scene.current.style);
        self.style_tuning
            .apply(&mut self.scene.current.target_style);
        if let Some(outgoing) = &mut self.scene.outgoing {
            self.style_tuning.apply(&mut outgoing.style);
            self.style_tuning.apply(&mut outgoing.target_style);
        }
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        self.analyzer.set_samplerate(samplerate);
        self.reset_audio();
    }

    /// Discard temporal history after capture restarts or the mode becomes visible.
    pub fn reset_audio(&mut self) {
        self.analyzer.reset();
        self.reset_analysis();
        self.pending_actions.clear();
        self.scene.reset_temporal();
        self.frames.clear();
        self.kick = 0.0;
        self.flip.reset(0.0);
        self.flip_target = 0.0;
        self.flip_count = 0;
        self.quake_x.reset(0.0);
        self.quake_y.reset(0.0);
        self.kick_sequence.reset();
        self.last_spectrum = Spectrum::default();
        self.analysis_revision = 0;
        self.kick_hits = 0;
        self.snare_hits = 0;
        self.drop_hits = 0;
        self.diagnosis.reset();
        self.debug_history.clear();
    }

    fn reset_analysis(&mut self) {
        for band in &mut self.bands {
            band.reset();
        }
        self.power.reset();
        self.bass.reset();
        self.treble.reset();
        self.onsets.reset();
    }

    /// Raw stereo frames feed the mono spectrum analyzer at the sample rate.
    pub fn consume_buf(&mut self, buf: &[[f32; 2]], gain: f32) {
        let hop = self.analyzer.hop_seconds();
        for frame in buf {
            let mono = (frame[0] + frame[1]) * 0.5 * gain;
            if let Some(spectrum) = self.analyzer.push(mono) {
                self.observe(spectrum, hop);
            }
        }
    }

    fn observe(&mut self, spectrum: Spectrum, hop: f32) {
        self.analysis_revision = self.analysis_revision.wrapping_add(1);
        self.last_spectrum = spectrum;
        let mut band_abdr = [0.0; BAND_COUNT];
        let mut band_abl = [0.0; BAND_COUNT];
        for (index, band) in self.bands.iter_mut().enumerate() {
            let value = spectrum.bands[index];
            let shape = &mut self.band_shapes[index];
            shape.track(value, hop);
            band.advance(shape.apply(value), hop);
            band_abdr[index] = (shape.high - shape.low).max(0.0);
            band_abl[index] = self.detectors.band_onset[index].loudness(shape.high);
        }
        let power = self.power_shape.observe(spectrum.power, hop);
        self.power.advance(power, hop);
        let bass = self.bass_shape.observe(spectrum.bass, hop);
        self.bass.advance(bass, hop);
        let treble = self.treble_shape.observe(spectrum.treble, hop);
        self.treble.advance(treble, hop);
        let events = self
            .onsets
            .observe(&spectrum, &band_abdr, &band_abl, &self.detectors, hop);
        if let Some(power) = events.drop {
            self.drop_hits = self.drop_hits.saturating_add(1);
            self.push_action(Action::Drop(power));
        }
        if events.snare.is_some() {
            self.snare_hits = self.snare_hits.saturating_add(1);
            self.push_action(Action::Snare);
        }
        if let Some(power) = events.kick {
            self.kick_hits = self.kick_hits.saturating_add(1);
            self.push_action(Action::Kick(power));
        }
    }

    fn mix_context(&self) -> MixContext {
        let status = self.onsets.status();
        MixContext {
            bands: self.bands.map(|band| band.value),
            raw_bands: self.last_spectrum.bands,
            power: self.power.value,
            power_level: self.last_spectrum.power,
            bass: self.bass.value,
            bass_level: self.last_spectrum.bass,
            kick: self.kick,
            kick_onset: status.kick.flash,
            snare_onset: status.snare.flash,
            drop_onset: status.drop.flash,
            gains: self.gains,
            routing: self.routing,
            style: self.scene.current.style,
            entrance: self.scene.current.entrance,
            phase: self.scene.current.phase,
            time: self.time,
        }
    }

    fn accent(&self) -> f32 {
        if self.property_gates.plays(Property::Accent) {
            self.mixer.value(MixPoint::Accent, &self.mix_context())
        } else {
            Property::Accent.neutral()
        }
    }

    fn push_action(&mut self, action: Action) {
        if self.pending_actions.len() == MAX_PENDING_ACTIONS {
            self.pending_actions.pop_front();
        }
        self.pending_actions.push_back(action);
    }

    pub fn frame(&self) -> Frame<'_> {
        Frame {
            lines: &self.lines,
            model_lines: &self.model_lines,
            ribbons: &self.ribbons,
            model_ribbons: &self.model_ribbons,
            model_composite: self.model_composite,
        }
    }

    /// Logical wireframe line count. Model edges stay counted while motion
    /// blur renders them as swept ribbons; diagnosis lines are included.
    pub fn line_count(&self) -> usize {
        if self.scene.current.is_showcase() {
            let outgoing = self
                .scene
                .outgoing
                .as_ref()
                .map_or(0, FigureRuntime::wireframe_edge_count);
            self.lines.len() + self.scene.current.wireframe_edge_count() + outgoing
        } else {
            self.lines.len()
        }
    }

    pub fn scene_name(&self) -> &str {
        match &self.showcase {
            Showcase::Failed(_) if self.presentation == Presentation::Weapon => "No model",
            _ => self.scene.current.name(),
        }
    }

    pub fn analysis_revision(&self) -> u64 {
        self.analysis_revision
    }

    pub fn palette(&self) -> Palette {
        self.palette
    }

    pub fn detector_settings(&self) -> DetectorSettings {
        self.detectors.clone()
    }

    pub fn set_detector_settings(&mut self, settings: DetectorSettings) {
        self.detectors = settings
            .validate()
            .expect("detector settings are validated before they reach the visualizer");
        self.analyzer.set_relative_excess(self.detectors.excess);
    }

    pub fn set_drop_kernel_band(&mut self, kernel: DropKernel, band: usize, value: f32) {
        self.detectors.set_drop_kernel_band(kernel, band, value);
    }

    pub fn hop_seconds(&self) -> f32 {
        self.analyzer.hop_seconds()
    }

    pub fn onset_history(&self) -> &VecDeque<[f32; BAND_COUNT]> {
        self.onsets.onset_history()
    }

    pub fn clear_pattern(&mut self, detector: Detector) {
        match detector {
            Detector::Kick => self.detectors.kick_pattern = TemporalPattern::kick_initial(),
            Detector::Snare => self.detectors.snare_pattern = TemporalPattern::snare_initial(),
            Detector::Drop | Detector::Flux => {
                unreachable!("only kick and snare have temporal patterns")
            }
        }
    }

    pub fn set_pattern_vertex(
        &mut self,
        detector: Detector,
        kernel: PatternKernel,
        layer: PatternLayer,
        vertex: usize,
        point: [f32; 2],
    ) {
        let pattern = match detector {
            Detector::Kick => &mut self.detectors.kick_pattern,
            Detector::Snare => &mut self.detectors.snare_pattern,
            Detector::Drop | Detector::Flux => {
                unreachable!("only kick and snare have temporal patterns")
            }
        };
        let polygon = match (kernel, layer) {
            (PatternKernel::Main, PatternLayer::Inner) => &mut pattern.inner,
            (PatternKernel::Main, PatternLayer::Outer) => &mut pattern.outer,
            (PatternKernel::Suppression, PatternLayer::Inner) => &mut pattern.suppression_inner,
            (PatternKernel::Suppression, PatternLayer::Outer) => &mut pattern.suppression_outer,
        };
        assert!(
            vertex < polygon.len(),
            "pattern vertices stay inside the polygon"
        );
        polygon[vertex] = [point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)];
    }

    pub fn insert_pattern_vertex(
        &mut self,
        detector: Detector,
        kernel: PatternKernel,
        layer: PatternLayer,
        edge: usize,
        point: [f32; 2],
    ) {
        let pattern = match detector {
            Detector::Kick => &mut self.detectors.kick_pattern,
            Detector::Snare => &mut self.detectors.snare_pattern,
            Detector::Drop | Detector::Flux => {
                unreachable!("only kick and snare have temporal patterns")
            }
        };
        let polygon = match (kernel, layer) {
            (PatternKernel::Main, PatternLayer::Inner) => &mut pattern.inner,
            (PatternKernel::Main, PatternLayer::Outer) => &mut pattern.outer,
            (PatternKernel::Suppression, PatternLayer::Inner) => &mut pattern.suppression_inner,
            (PatternKernel::Suppression, PatternLayer::Outer) => &mut pattern.suppression_outer,
        };
        assert!(
            edge < polygon.len(),
            "pattern edges stay inside the polygon"
        );
        polygon.insert(
            edge + 1,
            [point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)],
        );
    }

    pub fn remove_pattern_vertex(
        &mut self,
        detector: Detector,
        kernel: PatternKernel,
        layer: PatternLayer,
        vertex: usize,
    ) {
        let pattern = match detector {
            Detector::Kick => &mut self.detectors.kick_pattern,
            Detector::Snare => &mut self.detectors.snare_pattern,
            Detector::Drop | Detector::Flux => {
                unreachable!("only kick and snare have temporal patterns")
            }
        };
        let polygon = match (kernel, layer) {
            (PatternKernel::Main, PatternLayer::Inner) => &mut pattern.inner,
            (PatternKernel::Main, PatternLayer::Outer) => &mut pattern.outer,
            (PatternKernel::Suppression, PatternLayer::Inner) => &mut pattern.suppression_inner,
            (PatternKernel::Suppression, PatternLayer::Outer) => &mut pattern.suppression_outer,
        };
        assert!(
            polygon.len() > 3 && vertex < polygon.len(),
            "a polygon keeps a usable number of vertices"
        );
        polygon.remove(vertex);
    }

    pub fn routing(&self) -> &BandRouting {
        &self.routing
    }

    pub fn band_for(&self, component: BandComponent) -> usize {
        self.routing[component]
    }

    /// Current pre-envelope shaping for one FFT band.
    pub fn band_shape(&self, index: usize) -> InputShape {
        self.band_shapes[index]
    }

    /// Current pre-envelope shaping for one input component.
    pub fn input_shape(&self, input: Input) -> Option<InputShape> {
        match input {
            Input::Band(component) => Some(self.band_shapes[self.routing[component]]),
            Input::Power => Some(self.power_shape),
            Input::Bass => Some(self.bass_shape),
            _ => None,
        }
    }

    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// The static graph dependencies plus every analysis value an extra mix
    /// input feeds into this property.
    pub fn property_dependencies(&self, property: Property) -> Vec<Dependency> {
        let mut dependencies = property.dependencies().to_vec();
        for point in property.mix_points() {
            for (_, extra) in self.mixer.extras(*point) {
                for dependency in input_dependencies(extra.input) {
                    if !dependencies.contains(&dependency) {
                        dependencies.push(dependency);
                    }
                }
            }
        }
        dependencies
    }

    pub fn toggle_contribution_mix(&mut self, contribution: Contribution) -> Mix {
        self.mixer.toggle_mode(contribution)
    }

    pub fn add_extra(&mut self, point: MixPoint) -> ExtraRef {
        self.mixer.add_extra(point)
    }

    pub fn remove_extra(&mut self, extra: ExtraRef) {
        self.mixer.remove_extra(extra);
    }

    pub fn toggle_adaptive(&mut self, target: EnvelopeTarget) {
        self.shape_mut(target).toggle_adaptive();
    }

    pub fn toggle_property_mute(&mut self, property: Property) {
        self.property_gates.toggle_mute(property);
    }

    pub fn toggle_property_solo(&mut self, property: Property) {
        self.property_gates.toggle_solo(property);
    }

    pub fn property_muted(&self, property: Property) -> bool {
        self.property_gates.is_muted(property)
    }

    pub fn property_soloed(&self, property: Property) -> bool {
        self.property_gates.is_soloed(property)
    }

    /// Whether the property currently drives the scene instead of staying at
    /// its neutral value.
    pub fn property_active(&self, property: Property) -> bool {
        self.property_gates.plays(property)
    }

    fn onset_flux_value(&self, detector: Detector, setting: FluxSetting) -> f32 {
        let detectors = &self.detectors;
        match detector {
            Detector::Kick => match setting {
                FluxSetting::PreGain => detectors.kick_onset_pre_gain,
                FluxSetting::Attack => detectors.kick_onset_attack,
                FluxSetting::Release => detectors.kick_onset_release,
                FluxSetting::Average => detectors.kick_onset_average,
                FluxSetting::Sensitivity => detectors.kick_onset_sensitivity,
                FluxSetting::MinThreshold => detectors.kick_onset_min_threshold,
                FluxSetting::MinInterval => detectors.kick_onset_min_interval,
            },
            Detector::Snare => match setting {
                FluxSetting::PreGain => detectors.snare_onset_pre_gain,
                FluxSetting::Attack => detectors.snare_onset_attack,
                FluxSetting::Release => detectors.snare_onset_release,
                FluxSetting::Average => detectors.snare_onset_average,
                FluxSetting::Sensitivity => detectors.snare_onset_sensitivity,
                FluxSetting::MinThreshold => detectors.snare_onset_min_threshold,
                FluxSetting::MinInterval => detectors.snare_onset_min_interval,
            },
            Detector::Drop => match setting {
                FluxSetting::PreGain => detectors.drop_onset_pre_gain,
                FluxSetting::Attack => detectors.drop_onset_attack,
                FluxSetting::Release => detectors.drop_onset_release,
                FluxSetting::Average => detectors.drop_onset_average,
                FluxSetting::Sensitivity => detectors.drop_onset_sensitivity,
                FluxSetting::MinThreshold => detectors.drop_onset_min_threshold,
                FluxSetting::MinInterval => detectors.drop_onset_min_interval,
            },
            Detector::Flux => {
                unreachable!("the per-band onset analyzer uses its own parameters")
            }
        }
    }

    fn set_onset_flux_value(&mut self, detector: Detector, setting: FluxSetting, value: f32) {
        let detectors = &mut self.detectors;
        let slot = match detector {
            Detector::Kick => match setting {
                FluxSetting::PreGain => &mut detectors.kick_onset_pre_gain,
                FluxSetting::Attack => &mut detectors.kick_onset_attack,
                FluxSetting::Release => &mut detectors.kick_onset_release,
                FluxSetting::Average => &mut detectors.kick_onset_average,
                FluxSetting::Sensitivity => &mut detectors.kick_onset_sensitivity,
                FluxSetting::MinThreshold => &mut detectors.kick_onset_min_threshold,
                FluxSetting::MinInterval => &mut detectors.kick_onset_min_interval,
            },
            Detector::Snare => match setting {
                FluxSetting::PreGain => &mut detectors.snare_onset_pre_gain,
                FluxSetting::Attack => &mut detectors.snare_onset_attack,
                FluxSetting::Release => &mut detectors.snare_onset_release,
                FluxSetting::Average => &mut detectors.snare_onset_average,
                FluxSetting::Sensitivity => &mut detectors.snare_onset_sensitivity,
                FluxSetting::MinThreshold => &mut detectors.snare_onset_min_threshold,
                FluxSetting::MinInterval => &mut detectors.snare_onset_min_interval,
            },
            Detector::Drop => match setting {
                FluxSetting::PreGain => &mut detectors.drop_onset_pre_gain,
                FluxSetting::Attack => &mut detectors.drop_onset_attack,
                FluxSetting::Release => &mut detectors.drop_onset_release,
                FluxSetting::Average => &mut detectors.drop_onset_average,
                FluxSetting::Sensitivity => &mut detectors.drop_onset_sensitivity,
                FluxSetting::MinThreshold => &mut detectors.drop_onset_min_threshold,
                FluxSetting::MinInterval => &mut detectors.drop_onset_min_interval,
            },
            Detector::Flux => {
                unreachable!("the per-band onset analyzer uses its own parameters")
            }
        };
        *slot = value;
    }

    fn onset_suppression(&self, detector: Detector) -> f32 {
        match detector {
            Detector::Kick => self.detectors.kick_onset_suppression,
            Detector::Snare => self.detectors.snare_onset_suppression,
            Detector::Drop => self.detectors.drop_onset_suppression,
            Detector::Flux => {
                unreachable!("the per-band onset analyzer has no composite suppression")
            }
        }
    }

    fn set_onset_suppression(&mut self, detector: Detector, value: f32) {
        let slot = match detector {
            Detector::Kick => &mut self.detectors.kick_onset_suppression,
            Detector::Snare => &mut self.detectors.snare_onset_suppression,
            Detector::Drop => &mut self.detectors.drop_onset_suppression,
            Detector::Flux => {
                unreachable!("the per-band onset analyzer has no composite suppression")
            }
        };
        *slot = value;
    }

    /// Current value of one adjustable computation parameter.
    pub fn parameter_value(&self, parameter: Parameter) -> f32 {
        match parameter {
            Parameter::PatternArea(detector) => match detector {
                Detector::Kick => self.detectors.kick_pattern.effective,
                Detector::Snare => self.detectors.snare_pattern.effective,
                Detector::Drop | Detector::Flux => {
                    unreachable!("only kick and snare have temporal patterns")
                }
            },
            Parameter::BandAblGain(band) => self.detectors.band_onset[band].abl_gain,
            Parameter::BandAblCurve(band) => {
                self.detectors.band_onset[band].abl_curve.index() as f32
            }
            Parameter::BandAblAmount(band) => self.detectors.band_onset[band].abl_amount,
            Parameter::ExcessAttack => self.detectors.excess.background_attack,
            Parameter::ExcessRelease => self.detectors.excess.background_release,
            Parameter::ExcessFloor => self.detectors.excess.floor,
            Parameter::ExcessCap => self.detectors.excess.cap,
            Parameter::OnsetFlux(detector, setting) => self.onset_flux_value(detector, setting),
            Parameter::OnsetSuppression(detector) => self.onset_suppression(detector),
            Parameter::KickSnareChoke => self.detectors.kick_snare_choke,
            Parameter::KickDropChoke => self.detectors.kick_drop_choke,
            Parameter::KickQuakeFactor => self.detectors.kick_quake_factor,
            Parameter::KickSequenceEvery => self.detectors.kick_sequence_every as f32,
            Parameter::SnareFlipEvery => self.detectors.snare_flip_every as f32,
            Parameter::SnareChoke => self.detectors.snare_choke,
            Parameter::FlipStiffness => self.detectors.flip_stiffness,
            Parameter::FlipDamping => self.detectors.flip_damping,
            Parameter::DropGapWindow => self.detectors.drop_gap_window,
            Parameter::DropGapGain => self.detectors.drop_gap_gain,
            Parameter::DropGapThreshold => self.detectors.drop_gap_threshold,
            Parameter::DropLowLp => self.detectors.drop_low_lp,
            Parameter::DropHighLp => self.detectors.drop_high_lp,
            Parameter::DropQuakeFactor => self.detectors.drop_quake_factor,
            Parameter::QuakeStiffness => self.detectors.quake_stiffness,
            Parameter::QuakeDamping => self.detectors.quake_damping,
            Parameter::Attack(target) => self.envelope(target).attack,
            Parameter::Release(target) => self.envelope(target).release,
            Parameter::KickDecay => self.kick_decay,
            Parameter::Style(style) => style_value(&self.scene.current.style, style),
            Parameter::Gain(gain) => self.gains[gain],
            Parameter::BandSource(component) => self.routing[component] as f32,
            Parameter::InputLow(target) => self.shape(target).low,
            Parameter::InputHigh(target) => self.shape(target).high,
            Parameter::InputCurve(target) => self.shape(target).curve.index() as f32,
            Parameter::InputAmount(target) => self.shape(target).amount,
            Parameter::Constant(contribution) => self
                .mixer
                .constant_value(contribution)
                .expect("constant parameters only target constant terms"),
            Parameter::ExtraSource(extra) => self
                .mixer
                .extra(extra)
                .input
                .selectable_index()
                .expect("extra inputs are picked from the selectable list")
                as f32,
            Parameter::ExtraGain(extra) => self.mixer.extra(extra).gain,
        }
    }

    /// Applies one slider value, clamped to the parameter's declared range.
    pub fn set_parameter(&mut self, parameter: Parameter, value: f32) {
        let (minimum, maximum) = parameter.range();
        let value = value.clamp(minimum, maximum);
        match parameter {
            Parameter::PatternArea(detector) => {
                let pattern = match detector {
                    Detector::Kick => &mut self.detectors.kick_pattern,
                    Detector::Snare => &mut self.detectors.snare_pattern,
                    Detector::Drop | Detector::Flux => {
                        unreachable!("only kick and snare have temporal patterns")
                    }
                };
                pattern.effective = value;
            }
            Parameter::BandAblGain(band) => self.detectors.band_onset[band].abl_gain = value,
            Parameter::BandAblCurve(band) => {
                self.detectors.band_onset[band].abl_curve =
                    Curve::from_index(value.round() as usize);
            }
            Parameter::BandAblAmount(band) => self.detectors.band_onset[band].abl_amount = value,
            Parameter::ExcessAttack => {
                self.detectors.excess.background_attack = value;
                self.analyzer.set_relative_excess(self.detectors.excess);
            }
            Parameter::ExcessRelease => {
                self.detectors.excess.background_release = value;
                self.analyzer.set_relative_excess(self.detectors.excess);
            }
            Parameter::ExcessFloor => {
                self.detectors.excess.floor = value;
                self.analyzer.set_relative_excess(self.detectors.excess);
            }
            Parameter::ExcessCap => {
                self.detectors.excess.cap = value;
                self.analyzer.set_relative_excess(self.detectors.excess);
            }
            Parameter::OnsetFlux(detector, setting) => {
                self.set_onset_flux_value(detector, setting, value);
            }
            Parameter::OnsetSuppression(detector) => self.set_onset_suppression(detector, value),
            Parameter::KickSnareChoke => self.detectors.kick_snare_choke = value,
            Parameter::KickDropChoke => self.detectors.kick_drop_choke = value,
            Parameter::KickQuakeFactor => self.detectors.kick_quake_factor = value,
            Parameter::KickSequenceEvery => {
                self.detectors.kick_sequence_every = value.round() as u32
            }
            Parameter::SnareFlipEvery => self.detectors.snare_flip_every = value.round() as u32,
            Parameter::SnareChoke => self.detectors.snare_choke = value,
            Parameter::FlipStiffness => self.detectors.flip_stiffness = value,
            Parameter::FlipDamping => self.detectors.flip_damping = value,
            Parameter::DropGapWindow => self.detectors.drop_gap_window = value,
            Parameter::DropGapGain => self.detectors.drop_gap_gain = value,
            Parameter::DropGapThreshold => self.detectors.drop_gap_threshold = value,
            Parameter::DropLowLp => self.detectors.drop_low_lp = value,
            Parameter::DropHighLp => self.detectors.drop_high_lp = value,
            Parameter::DropQuakeFactor => self.detectors.drop_quake_factor = value,
            Parameter::QuakeStiffness => self.detectors.quake_stiffness = value,
            Parameter::QuakeDamping => self.detectors.quake_damping = value,
            Parameter::Attack(target) => self.envelope_mut(target).attack = value,
            Parameter::Release(target) => self.envelope_mut(target).release = value,
            Parameter::KickDecay => self.kick_decay = value,
            Parameter::Style(style) => {
                set_style_value(&mut self.scene.current.style, style, value);
                set_style_value(&mut self.scene.current.target_style, style, value);
                self.style_tuning.set(style, value);
            }
            Parameter::Gain(gain) => self.gains[gain] = value,
            Parameter::BandSource(component) => {
                self.routing[component] = (value.round() as usize).min(BAND_COUNT - 1)
            }
            Parameter::InputLow(target) => {
                let shape = self.shape_mut(target);
                shape.low = value.clamp(0.0, shape.high - 0.01);
            }
            Parameter::InputHigh(target) => {
                let shape = self.shape_mut(target);
                shape.high = value.clamp(shape.low + 0.01, 1.0);
            }
            Parameter::InputCurve(target) => {
                self.shape_mut(target).curve = Curve::from_index(value.round() as usize);
            }
            Parameter::InputAmount(target) => {
                self.shape_mut(target).amount = value;
            }
            Parameter::Constant(contribution) => {
                self.mixer.set_constant_value(contribution, value);
            }
            Parameter::ExtraSource(extra) => {
                self.mixer
                    .set_extra_input(extra, Input::from_selectable(value.round() as usize));
            }
            Parameter::ExtraGain(extra) => {
                self.mixer.set_extra_gain(extra, value);
            }
        }
    }

    fn envelope(&self, target: EnvelopeTarget) -> &Envelope {
        match target {
            EnvelopeTarget::Band(component) => &self.bands[self.routing[component]],
            EnvelopeTarget::BandIndex(band) => &self.bands[band],
            EnvelopeTarget::Power => &self.power,
            EnvelopeTarget::Bass => &self.bass,
            EnvelopeTarget::Treble => &self.treble,
        }
    }

    fn envelope_mut(&mut self, target: EnvelopeTarget) -> &mut Envelope {
        match target {
            EnvelopeTarget::Band(component) => &mut self.bands[self.routing[component]],
            EnvelopeTarget::BandIndex(band) => &mut self.bands[band],
            EnvelopeTarget::Power => &mut self.power,
            EnvelopeTarget::Bass => &mut self.bass,
            EnvelopeTarget::Treble => &mut self.treble,
        }
    }

    fn shape(&self, target: EnvelopeTarget) -> &InputShape {
        match target {
            EnvelopeTarget::Band(component) => &self.band_shapes[self.routing[component]],
            EnvelopeTarget::BandIndex(band) => &self.band_shapes[band],
            EnvelopeTarget::Power => &self.power_shape,
            EnvelopeTarget::Bass => &self.bass_shape,
            EnvelopeTarget::Treble => &self.treble_shape,
        }
    }

    fn shape_mut(&mut self, target: EnvelopeTarget) -> &mut InputShape {
        match target {
            EnvelopeTarget::Band(component) => &mut self.band_shapes[self.routing[component]],
            EnvelopeTarget::BandIndex(band) => &mut self.band_shapes[band],
            EnvelopeTarget::Power => &mut self.power_shape,
            EnvelopeTarget::Bass => &mut self.bass_shape,
            EnvelopeTarget::Treble => &mut self.treble_shape,
        }
    }

    pub fn depth_fade(&self) -> DepthFade {
        self.depth_fade
    }

    pub fn set_depth_fade(&mut self, depth_fade: DepthFade) {
        self.depth_fade = depth_fade;
    }

    pub fn near_opacity(&self) -> NearOpacity {
        self.near_opacity
    }

    pub fn set_near_opacity(&mut self, near_opacity: NearOpacity) {
        self.near_opacity = near_opacity;
    }

    pub fn set_crease_threshold(&mut self, crease: CreaseThreshold) {
        if self.crease_threshold == crease {
            return;
        }
        self.crease_threshold = crease;
        if let Showcase::Ready(models) = &self.showcase {
            if self.scene.current.is_showcase() {
                self.scene.set_showcase_wireframe(models.current(), crease);
            }
        }
    }

    pub fn set_model_composite(&mut self, composite: ModelComposite) {
        self.model_composite = composite;
    }

    pub fn set_line_width(&mut self, width: LineWidth) {
        self.line_width = width;
    }

    pub fn set_model_motion(&mut self, motion: ModelMotion) {
        if self.model_motion == motion {
            return;
        }
        self.model_motion = motion;
        if self.scene.current.is_showcase() {
            self.scene.set_model_motion(motion);
        }
    }

    pub fn set_motion_opacity(&mut self, opacity: MotionOpacity) {
        self.motion_opacity = opacity;
    }

    pub fn set_motion_fade_power(&mut self, power: MotionFadePower) {
        self.motion_fade_power = power;
    }

    pub fn diagnosis(&self) -> Option<&DiagnosisReadout> {
        if self.diagnosis.suppressed {
            None
        } else {
            self.diagnosis.readout.as_ref()
        }
    }

    /// Hides the random vertex diagnosis while the computation tree is open.
    pub fn set_diagnosis_suppressed(&mut self, suppressed: bool) {
        self.diagnosis.suppressed = suppressed;
    }

    pub fn diagnosis_anchor(&self) -> Option<[f32; 2]> {
        self.diagnosis
            .active
            .then_some([self.diagnosis.anchor.x, self.diagnosis.anchor.y])
    }

    fn property_values(&self) -> [f32; Property::COUNT] {
        let current = &self.scene.current;
        std::array::from_fn(|index| match Property::VARIANTS[index] {
            Property::Pulse => current.pose.scale,
            Property::Spin => current.spin_speed,
            Property::Pitch => current.pose.pitch,
            Property::Roll => current.pose.roll,
            Property::Twist => current.deformation.twist,
            Property::Bulge => current.deformation.bulge,
            Property::Ripple => current.deformation.ripple,
            Property::Shear => current.deformation.shear,
            Property::Zoom => current.camera_zoom,
            Property::Accent => self.accent(),
            Property::Line => current.line_energy,
            Property::Jitter => current.deformation.jitter,
            Property::Phase => current.phase_speed,
            Property::Kick => self.kick,
            Property::Flip => wrap_angle(self.flip.value()),
            Property::Quake => self.quake_x.value() + self.quake_y.value(),
            Property::Crossfade => {
                if self.scene.outgoing.is_some() {
                    self.scene.transition
                } else {
                    1.0
                }
            }
            Property::Morph => current.morph_rate,
            Property::Entrance => current.entrance,
        })
    }

    fn debug_sample(&self, at: f32) -> DebugSample {
        let status = self.onsets.status();
        DebugSample {
            at,
            bands: self.bands.map(|band| band.value),
            raw_bands: self.last_spectrum.bands,
            band_excess: std::array::from_fn(|index| status.bands[index].excess),
            band_onset: std::array::from_fn(|index| status.bands[index].onset),
            band_abdr: std::array::from_fn(|index| status.bands[index].abdr),
            band_abl: std::array::from_fn(|index| status.bands[index].abl),
            band_drp: std::array::from_fn(|index| status.bands[index].drp),
            power: self.power.value,
            raw_power: self.last_spectrum.power,
            bass: self.bass.value,
            raw_bass: self.last_spectrum.bass,
            kick: self.kick,
            kick_onset: status.kick,
            snare_onset: status.snare,
            drop_onset: status.drop,
            drop_status: status.drop_status,
            kick_flux: status.kick_flux,
            snare_flux: status.snare_flux,
            properties: self.property_values(),
        }
    }

    fn record_debug_sample(&mut self) {
        if self
            .debug_history
            .latest()
            .is_some_and(|sample| self.time - sample.at < DEBUG_SAMPLE_INTERVAL)
        {
            return;
        }
        let sample = self.debug_sample(self.time);
        self.debug_history.samples.push_back(sample);
        while self
            .debug_history
            .samples
            .front()
            .is_some_and(|sample| self.time - sample.at > DEBUG_HISTORY_SECONDS)
        {
            self.debug_history.samples.pop_front();
        }
    }

    pub fn debug_snapshot(&self) -> DebugSnapshot {
        let status = self.onsets.status();
        let bands = std::array::from_fn(|index| {
            let (low_hz, high_hz) = self.analyzer.band_range(index);
            BandDebug {
                value: self.bands[index].value,
                raw: self.last_spectrum.bands[index],
                excess: status.bands[index].excess,
                onset: status.bands[index].onset,
                abdr: status.bands[index].abdr,
                abl: status.bands[index].abl,
                drp: status.bands[index].drp,
                low_hz,
                high_hz,
                role: BandRole::for_index(index),
            }
        });
        let style = self.scene.current.style;
        DebugSnapshot {
            bands,
            properties: self.property_values(),
            power: EnvelopeDebug {
                smooth: self.power.value,
                raw: self.last_spectrum.power,
            },
            bass: EnvelopeDebug {
                smooth: self.bass.value,
                raw: self.last_spectrum.bass,
            },
            treble: EnvelopeDebug {
                smooth: self.treble.value,
                raw: self.last_spectrum.treble,
            },
            kick: self.kick,
            kick_onset: status.kick,
            snare_onset: status.snare,
            drop_onset: status.drop,
            drop_status: status.drop_status,
            kick_flux: status.kick_flux,
            snare_flux: status.snare_flux,
            kick_hits: self.kick_hits,
            snare_hits: self.snare_hits,
            drop_hits: self.drop_hits,
            scene: self.scene_name().to_owned(),
            outgoing: self
                .scene
                .outgoing
                .as_ref()
                .map(|figure| figure.name().to_owned()),
            transition: self.scene.transition,
            style: StyleDebug {
                axis: [style.axis.x, style.axis.y, style.axis.z],
                twist_sign: style.twist_sign,
                shear_sign: style.shear_sign,
                deform_bias: style.deform_bias,
                zoom_bias: style.zoom_bias,
                phase_rate: style.phase_rate,
                jitter_bias: style.jitter_bias,
            },
            phase: self.scene.current.phase.rem_euclid(TAU),
            entrance: self.scene.current.entrance,
            lines: self.lines.len() + self.model_lines.len(),
            ribbons: self.ribbons.len() + self.model_ribbons.len(),
        }
    }

    /// Rolling display-rate history feeding the debug inspector graphs.
    pub fn debug_history(&self) -> &DebugHistory {
        &self.debug_history
    }

    /// Advance the scene from the latest analysis and rebuild the line overlay.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        frame: AudioInterval,
        shutter: ShutterFrames,
        dt: f32,
        palette: &Palette,
        viewport: (f32, f32),
        zoom: f32,
        ui_scale: f32,
    ) {
        let dt = dt.clamp(0.0, 0.1);
        if self.presentation == Presentation::Weapon && matches!(&self.showcase, Showcase::Unloaded)
        {
            self.enter_showcase();
        }
        self.palette.approach(*palette, dt, 10.0);
        self.time += dt;
        let accent = self.accent();
        while let Some(action) = self.pending_actions.pop_front() {
            self.handle_action(action, accent);
        }
        self.kick *= (-dt * self.kick_decay).exp();
        if !self.property_gates.plays(Property::Kick) {
            self.kick = 0.0;
        }
        self.flip.set_target(self.flip_target);
        self.flip.advance(
            dt,
            self.detectors.flip_stiffness,
            self.detectors.flip_damping,
        );
        self.quake_x.advance(
            dt,
            self.detectors.quake_stiffness,
            self.detectors.quake_damping,
        );
        self.quake_y.advance(
            dt,
            self.detectors.quake_stiffness,
            self.detectors.quake_damping,
        );
        let status = self.onsets.status();
        let exposure = self.frames.interval(frame, shutter);
        let camera = Camera::new(viewport.0, viewport.1, zoom);
        let music = Music {
            bands: self.bands.map(|band| band.value),
            raw_bands: self.last_spectrum.bands,
            power: self.power.value,
            power_level: self.last_spectrum.power,
            bass: self.bass.value,
            bass_level: self.last_spectrum.bass,
            kick: self.kick,
            kick_onset: status.kick.flash,
            snare_onset: status.snare.flash,
            drop_onset: status.drop.flash,
        };
        let width =
            (viewport.0.min(viewport.1) / 440.0).clamp(0.62, 2.6) * 1.4 * self.line_width.factor();
        let quake_scale = (viewport.0.min(viewport.1) / 440.0).clamp(0.62, 2.6) * QUAKE_PIXELS;
        let flip_pitch = if self.property_gates.plays(Property::Flip) {
            self.flip.value()
        } else {
            0.0
        };
        let shake = if self.property_gates.plays(Property::Quake) {
            Vec2::new(
                self.quake_x.value() * quake_scale,
                self.quake_y.value() * quake_scale,
            )
        } else {
            Vec2::new(0.0, 0.0)
        };
        self.lines.clear();
        self.model_lines.clear();
        self.ribbons.clear();
        self.model_ribbons.clear();
        if self.presentation == Presentation::Weapon
            && matches!(&self.showcase, Showcase::Failed(_))
        {
            self.diagnosis.active = false;
            self.diagnosis.readout = None;
            self.record_debug_sample();
            return;
        }
        let showcase = self.scene.current.is_showcase();
        let (scene_lines, scene_ribbons) = if showcase {
            (&mut self.model_lines, &mut self.model_ribbons)
        } else {
            (&mut self.lines, &mut self.ribbons)
        };
        self.scene.update(
            dt,
            &music,
            &self.gains,
            &self.routing,
            &self.mixer,
            &self.property_gates,
            exposure.start(),
            frame.end(),
            camera,
            &self.palette,
            self.depth_fade.strength(),
            self.near_opacity.value(),
            MotionFade {
                opacity: self.motion_opacity,
                power: self.motion_fade_power,
            },
            width,
            flip_pitch,
            shake,
            self.time,
            scene_lines,
            scene_ribbons,
        );
        self.record_debug_sample();
        let ui_scale = ui_scale.max(0.25);
        self.update_diagnosis(dt, viewport, self.palette, ui_scale);
        self.append_diagnosis(viewport, ui_scale);
    }

    fn diagnosis_target(&self, viewport: (f32, f32), ui_scale: f32) -> Vec2 {
        let vertex = self
            .scene
            .current
            .vertex_diagnosis(self.diagnosis.vertex)
            .map(|data| data.screen)
            .unwrap_or(Vec2::new(viewport.0 * 0.5, viewport.1 * 0.5));
        let center = Vec2::new(viewport.0 * 0.5, viewport.1 * 0.5);
        let side = if vertex.x < center.x { 1.0 } else { -1.0 };
        let target = Vec2::new(
            vertex.x + side * 150.0 * ui_scale,
            vertex.y - 70.0 * ui_scale,
        );
        Vec2::new(
            target
                .x
                .clamp(36.0 * ui_scale, (viewport.0 * 0.36).max(36.0 * ui_scale)),
            target.y.clamp(
                36.0 * ui_scale,
                (viewport.1 - 36.0 * ui_scale).max(36.0 * ui_scale),
            ),
        )
    }

    fn activate_diagnosis(&mut self, viewport: (f32, f32), ui_scale: f32) {
        let count = self.scene.current.vertices.len();
        if count == 0 {
            return;
        }
        self.diagnosis.active = true;
        self.diagnosis.vertex = self.rng.range(count);
        self.diagnosis.mode = DiagnosisMode::random(&mut self.rng);
        self.diagnosis.remaining = 2.5 + self.rng.unit() * 3.0;
        self.diagnosis.mode_timer = 0.0;
        self.diagnosis.refresh_timer = 0.1;
        self.diagnosis.scene = self.scene.current.name().to_owned();
        let target = self.diagnosis_target(viewport, ui_scale);
        self.diagnosis.anchor = target;
        self.diagnosis.revision += 1;
    }

    fn update_diagnosis(&mut self, dt: f32, viewport: (f32, f32), palette: Palette, ui_scale: f32) {
        if !self.diagnosis.active {
            self.diagnosis.next_in -= dt;
            if self.diagnosis.next_in <= 0.0 {
                self.activate_diagnosis(viewport, ui_scale);
            }
            return;
        }
        self.diagnosis.remaining -= dt;
        if self.diagnosis.remaining <= 0.0 {
            self.diagnosis.active = false;
            self.diagnosis.readout = None;
            self.diagnosis.revision += 1;
            self.diagnosis.next_in = 3.0 + self.rng.unit() * 5.0;
            return;
        }
        let scene = self.scene.current.name();
        if scene != self.diagnosis.scene {
            self.diagnosis.scene = scene.to_owned();
            self.diagnosis.vertex = self.rng.range(self.scene.current.vertices.len().max(1));
            self.diagnosis.mode = DiagnosisMode::random(&mut self.rng);
            self.diagnosis.mode_timer = 0.0;
            self.diagnosis.refresh_timer = 0.1;
            let target = self.diagnosis_target(viewport, ui_scale);
            self.diagnosis.anchor = target;
            self.diagnosis.readout = None;
            self.diagnosis.revision += 1;
        }
        if self.diagnosis.vertex >= self.scene.current.vertices.len() {
            self.diagnosis.vertex = self.scene.current.vertices.len().saturating_sub(1);
        }
        self.diagnosis.mode_timer += dt;
        if self.diagnosis.mode_timer >= 1.8 {
            self.diagnosis.mode_timer = 0.0;
            self.diagnosis.mode = self.diagnosis.mode.next();
            self.diagnosis.refresh_timer = 0.1;
            self.diagnosis.revision += 1;
        }
        let target = self.diagnosis_target(viewport, ui_scale);
        self.diagnosis.anchor =
            self.diagnosis.anchor + (target - self.diagnosis.anchor) * (1.0 - (-dt * 0.35).exp());
        self.diagnosis.refresh_timer += dt;
        if self.diagnosis.refresh_timer >= 0.1 {
            self.diagnosis.refresh_timer = 0.0;
            if let Some(data) = self.scene.current.vertex_diagnosis(self.diagnosis.vertex) {
                let color = palette.vivid(self.diagnosis.vertex + self.diagnosis.mode.index() * 4);
                self.diagnosis.revision += 1;
                self.diagnosis.readout = Some(DiagnosisReadout {
                    revision: self.diagnosis.revision,
                    text: self.diagnosis.mode.format(data),
                    color,
                });
            }
        }
    }

    fn append_diagnosis(&mut self, viewport: (f32, f32), ui_scale: f32) {
        if !self.diagnosis.active || self.diagnosis.suppressed {
            return;
        }
        let Some(data) = self.scene.current.vertex_diagnosis(self.diagnosis.vertex) else {
            return;
        };
        let Some(readout) = &self.diagnosis.readout else {
            return;
        };
        let color = readout.color.components;
        let vertex = data.screen;
        let anchor = self.diagnosis.anchor;
        let size = (viewport.0.min(viewport.1) / 80.0).clamp(7.0, 16.0) * ui_scale;
        let half = size * 0.5;
        let arm = size * 0.34;
        let width = 1.15 * ui_scale;
        let alpha = 0.78;
        let mut push = |a: Vec2, b: Vec2| {
            self.lines.push(LineInstance::new(
                a,
                b,
                [color[0], color[1], color[2], alpha],
                width,
            ));
        };
        // Four sci-fi corner brackets.
        push(
            Vec2::new(vertex.x - half, vertex.y - half),
            Vec2::new(vertex.x - half + arm, vertex.y - half),
        );
        push(
            Vec2::new(vertex.x - half, vertex.y - half),
            Vec2::new(vertex.x - half, vertex.y - half + arm),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y - half),
            Vec2::new(vertex.x + half - arm, vertex.y - half),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y - half),
            Vec2::new(vertex.x + half, vertex.y - half + arm),
        );
        push(
            Vec2::new(vertex.x - half, vertex.y + half),
            Vec2::new(vertex.x - half + arm, vertex.y + half),
        );
        push(
            Vec2::new(vertex.x - half, vertex.y + half),
            Vec2::new(vertex.x - half, vertex.y + half - arm),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y + half),
            Vec2::new(vertex.x + half - arm, vertex.y + half),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y + half),
            Vec2::new(vertex.x + half, vertex.y + half - arm),
        );
        // Small crosshair ticks.
        push(
            Vec2::new(vertex.x, vertex.y - half * 0.55),
            Vec2::new(vertex.x, vertex.y - half * 1.55),
        );
        push(
            Vec2::new(vertex.x, vertex.y + half * 0.55),
            Vec2::new(vertex.x, vertex.y + half * 1.55),
        );
        push(
            Vec2::new(vertex.x - half * 0.55, vertex.y),
            Vec2::new(vertex.x - half * 1.55, vertex.y),
        );
        push(
            Vec2::new(vertex.x + half * 0.55, vertex.y),
            Vec2::new(vertex.x + half * 1.55, vertex.y),
        );
        // Stepped polyline to the lazily following readout.
        let side = if anchor.x < vertex.x { -1.0 } else { 1.0 };
        let p1 = Vec2::new(vertex.x + side * 18.0 * ui_scale, vertex.y);
        let p2 = Vec2::new(anchor.x + side * 24.0 * ui_scale, vertex.y);
        let p3 = Vec2::new(anchor.x + side * 24.0 * ui_scale, anchor.y);
        push(vertex, p1);
        push(p1, p2);
        push(p2, p3);
        push(p3, anchor);
        push(
            Vec2::new(anchor.x - 4.0 * ui_scale, anchor.y),
            Vec2::new(anchor.x + 4.0 * ui_scale, anchor.y),
        );
    }

    fn handle_action(&mut self, action: Action, accent: f32) {
        match action {
            Action::Kick(power) => {
                self.kick = self.kick.max(power.clamp(0.0, 1.5));
                if self.property_gates.plays(Property::Quake) {
                    let direction = self.kick_sequence.next(self.detectors.kick_sequence_every);
                    self.quake_impulse(direction, power * self.detectors.kick_quake_factor);
                }
            }
            Action::Snare => {
                self.kick_sequence.reset();
                self.flip_count += 1;
                if self.flip_count >= self.detectors.snare_flip_every {
                    self.flip_count = 0;
                    if self.property_gates.plays(Property::Flip) {
                        self.flip_target += TAU * 0.5;
                    }
                }
            }
            Action::Drop(power) => {
                if self.scene.current.is_showcase() {
                    self.advance_model(accent);
                } else {
                    self.scene.change(&mut self.rng, accent, &self.style_tuning);
                }
                if self.property_gates.plays(Property::Quake) {
                    self.quake_y
                        .impulse(power * self.detectors.drop_quake_factor);
                }
            }
        }
    }

    fn quake_impulse(&mut self, direction: QuakeDirection, velocity: f32) {
        let (x, y) = direction.unit();
        self.quake_x.impulse(x * velocity);
        self.quake_y.impulse(y * velocity);
    }

    #[cfg(test)]
    fn debug_music(&mut self, bands: [f32; BAND_COUNT], power: f32) {
        for (envelope, value) in self.bands.iter_mut().zip(bands) {
            envelope.value = value;
        }
        self.power.value = power;
        self.bass.value = bands[..4].iter().sum::<f32>() / 4.0;
    }
}

impl Default for Visualizer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct Music {
    bands: [f32; BAND_COUNT],
    raw_bands: [f32; BAND_COUNT],
    power: f32,
    power_level: f32,
    bass: f32,
    bass_level: f32,
    kick: f32,
    kick_onset: f32,
    snare_onset: f32,
    drop_onset: f32,
}

impl Music {
    fn context(
        self,
        style: Style,
        gains: Gains,
        routing: BandRouting,
        entrance: f32,
        phase: f32,
        time: f32,
    ) -> MixContext {
        MixContext {
            bands: self.bands,
            raw_bands: self.raw_bands,
            power: self.power,
            power_level: self.power_level,
            bass: self.bass,
            bass_level: self.bass_level,
            kick: self.kick,
            kick_onset: self.kick_onset,
            snare_onset: self.snare_onset,
            drop_onset: self.drop_onset,
            gains,
            routing,
            style,
            entrance,
            phase,
            time,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Kick(f32),
    Snare,
    Drop(f32),
}

/// Folds an unbounded angle into the closest representation around zero.
fn wrap_angle(angle: f32) -> f32 {
    (angle + TAU * 0.5).rem_euclid(TAU) - TAU * 0.5
}

#[derive(Clone, Copy)]
struct Envelope {
    value: f32,
    attack: f32,
    release: f32,
}

impl Envelope {
    fn new(attack: f32, release: f32) -> Self {
        assert!(attack > 0.0 && release > 0.0);
        Self {
            value: 0.0,
            attack,
            release,
        }
    }

    fn advance(&mut self, target: f32, dt: f32) {
        let rate = if target > self.value {
            self.attack
        } else {
            self.release
        };
        let blend = 1.0 - (-rate * dt).exp();
        self.value += (target - self.value) * blend;
    }

    fn reset(&mut self) {
        self.value = 0.0;
    }
}

#[derive(Clone, Copy)]
struct Smoother {
    value: f32,
}

impl Smoother {
    fn new(value: f32) -> Self {
        Self { value }
    }

    fn approach(&mut self, target: f32, dt: f32, rate: f32) -> f32 {
        let blend = 1.0 - (-rate * dt).exp();
        self.value += (target - self.value) * blend;
        self.value
    }
}

struct Scene {
    current: FigureRuntime,
    outgoing: Option<FigureRuntime>,
    transition: f32,
}

impl Scene {
    fn new(rng: &mut Rng) -> Self {
        let style = Style::default();
        let tint = FigureTint::spawn(FigureKind::Cube, 0.0, rng);
        Self {
            current: FigureRuntime::new(FigureKind::Cube, 1.0, style, tint),
            outgoing: None,
            transition: 1.0,
        }
    }

    fn change(&mut self, rng: &mut Rng, accent: f32, style_tuning: &StyleTuning) {
        let next = match &self.current.source {
            FigureSource::Procedural(kind) => kind.next(),
            FigureSource::Model { .. } => return,
        };
        let mut target_style = Style::random(rng);
        style_tuning.apply(&mut target_style);
        let tint = FigureTint::spawn(next, accent, rng);
        let mut entering = FigureRuntime::new(next, 0.0, target_style, tint);
        entering.pose.yaw += rng.signed() * 0.9;
        entering.pose.pitch += rng.signed() * 0.4;
        entering.phase = rng.unit() * TAU;
        self.outgoing = None;
        self.current.opacity = 1.0;
        let old = std::mem::replace(&mut self.current, entering);
        self.outgoing = Some(old);
        self.transition = 0.0;
    }

    fn enter_showcase(
        &mut self,
        model: &model::Model,
        crease: CreaseThreshold,
        motion: ModelMotion,
        accent: f32,
        style_tuning: &StyleTuning,
    ) {
        let mut style = Style::default();
        style_tuning.apply(&mut style);
        self.current = FigureRuntime::showcase(
            model.name.clone(),
            model.wireframe(crease),
            style,
            FigureTint::solid(accent),
            motion,
        );
        self.outgoing = None;
        self.transition = 1.0;
    }

    fn set_showcase_wireframe(&mut self, model: &model::Model, crease: CreaseThreshold) {
        assert!(
            self.current.is_showcase(),
            "only the weapon showcase changes its crease threshold"
        );
        self.current.set_wireframe(model.wireframe(crease));
    }

    /// Crossfades into another model while keeping the current orientation.
    fn switch_showcase(
        &mut self,
        model: &model::Model,
        crease: CreaseThreshold,
        motion: ModelMotion,
        accent: f32,
    ) {
        assert!(
            self.current.is_showcase(),
            "only the weapon showcase switches models"
        );
        let mut entering = FigureRuntime::showcase(
            model.name.clone(),
            model.wireframe(crease),
            self.current.target_style,
            FigureTint::solid(accent),
            motion,
        );
        entering.pose = self.current.pose;
        entering.pitch = self.current.pitch;
        entering.roll = self.current.roll;
        entering.style = self.current.style;
        entering.phase = self.current.phase;
        self.outgoing = Some(std::mem::replace(&mut self.current, entering));
        self.transition = 0.0;
    }

    fn set_model_motion(&mut self, motion: ModelMotion) {
        assert!(
            self.current.is_showcase(),
            "only the weapon showcase changes its motion blur"
        );
        self.current.set_motion_blur(motion == ModelMotion::Swept);
        if let Some(outgoing) = &mut self.outgoing {
            assert!(
                outgoing.is_showcase(),
                "the outgoing showcase model owns its motion blur"
            );
            outgoing.set_motion_blur(motion == ModelMotion::Swept);
        }
    }

    fn leave_showcase(&mut self, rng: &mut Rng, accent: f32, style_tuning: &StyleTuning) {
        let mut style = Style::random(rng);
        style_tuning.apply(&mut style);
        self.current = FigureRuntime::new(
            FigureKind::Cube,
            1.0,
            style,
            FigureTint::spawn(FigureKind::Cube, accent, rng),
        );
        self.outgoing = None;
        self.transition = 1.0;
    }

    fn reset_temporal(&mut self) {
        self.current.reset_temporal();
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.reset_temporal();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        dt: f32,
        music: &Music,
        gains: &Gains,
        routing: &BandRouting,
        mixer: &Mixer,
        gates: &PropertyGates,
        exposure_start: AudioTime,
        frame_end: AudioTime,
        camera: Camera,
        palette: &Palette,
        depth_fade: f32,
        near_opacity: f32,
        motion_fade: MotionFade,
        width: f32,
        flip_pitch: f32,
        shake: Vec2,
        time: f32,
        out: &mut Vec<LineInstance>,
        out_ribbons: &mut Vec<RibbonInstance>,
    ) {
        if self.outgoing.is_some() && gates.plays(Property::Crossfade) {
            let context = music.context(
                self.current.style,
                *gains,
                *routing,
                self.current.entrance,
                self.current.phase,
                time,
            );
            let rate = mixer.value(MixPoint::CrossfadeRate, &context);
            self.transition = (self.transition + dt * rate).min(1.0);
            let progress = smoothstep(self.transition);
            if let Some(outgoing) = &mut self.outgoing {
                outgoing.opacity = 1.0 - progress;
            }
            self.current.opacity = progress;
        } else {
            self.current.opacity = 1.0;
            if self.outgoing.is_some() {
                // A muted crossfade swaps scenes without a fade.
                self.transition = Property::Crossfade.neutral();
                self.outgoing = None;
            }
        }
        self.current.update(
            dt,
            music,
            gains,
            routing,
            mixer,
            gates,
            exposure_start,
            frame_end,
            camera,
            palette,
            depth_fade,
            near_opacity,
            motion_fade,
            width,
            flip_pitch,
            shake,
            time,
            out,
            out_ribbons,
        );
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.update(
                dt,
                music,
                gains,
                routing,
                mixer,
                gates,
                exposure_start,
                frame_end,
                camera,
                palette,
                depth_fade,
                near_opacity,
                motion_fade,
                width,
                flip_pitch,
                shake,
                time,
                out,
                out_ribbons,
            );
        }
        if self.transition >= 1.0 {
            self.outgoing = None;
        }
    }
}

/// What the runtime draws: one step of the figure evolution, or a loaded model.
enum FigureSource {
    Procedural(FigureKind),
    Model { name: String },
}

impl FigureSource {
    fn name(&self) -> &str {
        match self {
            Self::Procedural(kind) => kind.name(),
            Self::Model { name } => name,
        }
    }

    fn is_model(&self) -> bool {
        matches!(self, Self::Model { .. })
    }
}

struct FigureRuntime {
    source: FigureSource,
    wireframe: Wireframe,
    pose: Pose,
    pitch: Smoother,
    roll: Smoother,
    style: Style,
    target_style: Style,
    tint: FigureTint,
    phase: f32,
    /// Latest values written by `update` and read by the debug inspector.
    deformation: Deformation,
    spin_speed: f32,
    phase_speed: f32,
    camera_zoom: f32,
    morph_rate: f32,
    line_energy: f32,
    entrance: f32,
    opacity: f32,
    motion_blur: bool,
    history: VecDeque<PoseSnapshot>,
    vertices: Vec<Projected>,
    world_vertices: Vec<Vec3>,
    previous_screen: Vec<Vec2>,
    velocity: Vec<Vec2>,
    acceleration: Vec<Vec2>,
    current: Vec<LineGeometry>,
}

#[derive(Clone)]
struct PoseSnapshot {
    at: AudioTime,
    lines: Vec<LineGeometry>,
}

#[derive(Clone, Copy)]
struct LineGeometry {
    start: Vec2,
    end: Vec2,
    start_depth_opacity: f32,
    end_depth_opacity: f32,
}

#[derive(Clone, Copy)]
struct VertexDiagnosis {
    screen: Vec2,
    world: Vec3,
    velocity: Vec2,
    acceleration: Vec2,
}

impl FigureRuntime {
    fn new(kind: FigureKind, opacity: f32, target_style: Style, tint: FigureTint) -> Self {
        Self::with_source(
            FigureSource::Procedural(kind),
            Wireframe::build(kind),
            Pose::default(),
            opacity,
            target_style,
            tint,
            true,
        )
    }

    /// A loaded model stays rigid and keeps a permanent label instead of
    /// evolving through the figure kinds.
    fn showcase(
        name: String,
        wireframe: Wireframe,
        target_style: Style,
        tint: FigureTint,
        motion: ModelMotion,
    ) -> Self {
        let pose = Pose {
            yaw: TAU * 0.25,
            ..Pose::default()
        };
        Self::with_source(
            FigureSource::Model { name },
            wireframe,
            pose,
            1.0,
            target_style,
            tint,
            motion == ModelMotion::Swept,
        )
    }

    fn with_source(
        source: FigureSource,
        wireframe: Wireframe,
        pose: Pose,
        opacity: f32,
        target_style: Style,
        tint: FigureTint,
        motion_blur: bool,
    ) -> Self {
        let vertex_capacity = wireframe.vertex_count();
        let edge_capacity = wireframe.edges().len();
        Self {
            source,
            wireframe,
            pose,
            pitch: Smoother::new(pose.pitch),
            roll: Smoother::new(pose.roll),
            style: Style::default(),
            target_style,
            tint,
            phase: 0.0,
            deformation: Deformation::default(),
            spin_speed: 0.0,
            phase_speed: 0.0,
            camera_zoom: 1.0,
            morph_rate: 1.1,
            line_energy: 0.6,
            entrance: opacity,
            opacity,
            motion_blur,
            history: VecDeque::with_capacity(MAX_HISTORY),
            vertices: Vec::with_capacity(vertex_capacity),
            world_vertices: Vec::with_capacity(vertex_capacity),
            previous_screen: Vec::with_capacity(vertex_capacity),
            velocity: Vec::with_capacity(vertex_capacity),
            acceleration: Vec::with_capacity(vertex_capacity),
            current: Vec::with_capacity(edge_capacity),
        }
    }

    fn name(&self) -> &str {
        self.source.name()
    }

    fn wireframe_edge_count(&self) -> usize {
        self.wireframe.edges().len()
    }

    fn is_showcase(&self) -> bool {
        self.source.is_model()
    }

    fn set_motion_blur(&mut self, enabled: bool) {
        assert!(self.is_showcase(), "figure motion blur is always enabled");
        self.motion_blur = enabled;
        if !enabled {
            self.history.clear();
        }
    }

    fn set_wireframe(&mut self, wireframe: Wireframe) {
        assert!(
            self.is_showcase(),
            "only the weapon showcase changes its wireframe at runtime"
        );
        self.wireframe = wireframe;
        self.history.clear();
        self.current.clear();
        self.vertices.clear();
        self.world_vertices.clear();
        self.previous_screen.clear();
        self.velocity.clear();
        self.acceleration.clear();
    }

    fn reset_temporal(&mut self) {
        self.history.clear();
    }

    // Sixteen log-spaced areas drive the scene: 0-3 bass pulse and hits, 4 spin,
    // 5 pitch, 6 roll, 7 twist, 8 bulge, 9 ripple, 10 shear, 11 camera zoom,
    // 12 accent saturation of newly spawned figures, 13 line energy, 14 jitter,
    // 15 phase speed.
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        dt: f32,
        music: &Music,
        gains: &Gains,
        routing: &BandRouting,
        mixer: &Mixer,
        gates: &PropertyGates,
        exposure_start: AudioTime,
        frame_end: AudioTime,
        camera: Camera,
        palette: &Palette,
        depth_fade: f32,
        near_opacity: f32,
        motion_fade: MotionFade,
        base_width: f32,
        flip_pitch: f32,
        shake: Vec2,
        time: f32,
        out: &mut Vec<LineInstance>,
        out_ribbons: &mut Vec<RibbonInstance>,
    ) {
        let context = music.context(
            self.style,
            *gains,
            *routing,
            self.entrance,
            self.phase,
            time,
        );
        let entrance_rate = mixer.value(MixPoint::EntranceRate, &context);
        self.entrance = if gates.plays(Property::Entrance) {
            (self.entrance + dt * entrance_rate).min(1.0)
        } else {
            Property::Entrance.neutral()
        };
        self.morph_rate = gates.drive(Property::Morph, mixer.value(MixPoint::MorphRate, &context));
        self.style.approach(self.target_style, dt, self.morph_rate);
        let context = music.context(
            self.style,
            *gains,
            *routing,
            self.entrance,
            self.phase,
            time,
        );
        let spin = gates.drive(Property::Spin, mixer.value(MixPoint::SpinSpeed, &context));
        self.spin_speed = spin;
        self.pose.yaw =
            (self.pose.yaw + dt * spin * (0.3 + self.style.axis.x.abs())).rem_euclid(TAU);
        let pitch_target = gates.drive(
            Property::Pitch,
            mixer.value(MixPoint::PitchTarget, &context),
        );
        let pitch_rate = mixer.value(MixPoint::PitchRate, &context);
        self.pose.pitch = self.pitch.approach(pitch_target, dt, pitch_rate);
        let roll_target = gates.drive(Property::Roll, mixer.value(MixPoint::RollTarget, &context));
        let roll_rate = mixer.value(MixPoint::RollRate, &context);
        self.pose.roll = self.roll.approach(roll_target, dt, roll_rate);
        self.phase_speed =
            gates.drive(Property::Phase, mixer.value(MixPoint::PhaseSpeed, &context));
        self.phase = (self.phase + dt * self.phase_speed).rem_euclid(PHASE_PERIOD);
        // The figure scale combines the pulse channel with the separate camera
        // zoom property, so either can be muted on its own.
        let entrance_scale = gates.drive(
            Property::Pulse,
            mixer.value(MixPoint::PulseEntrance, &context),
        );
        let camera_zoom = gates.drive(Property::Zoom, mixer.value(MixPoint::PulseCamera, &context));
        self.camera_zoom = camera_zoom;
        let energy = gates.drive(
            Property::Pulse,
            mixer.value(MixPoint::PulseEnergy, &context),
        );
        let scale = entrance_scale * energy * self.style.zoom_bias * camera_zoom;
        self.pose.scale = soft_saturate(scale, 1.45).max(0.35);
        let showcase = self.is_showcase();
        let deformation = if showcase {
            Deformation::default()
        } else {
            Deformation {
                twist: gates.drive(Property::Twist, mixer.value(MixPoint::Twist, &context)),
                bulge: gates.drive(Property::Bulge, mixer.value(MixPoint::Bulge, &context)),
                ripple: gates.drive(Property::Ripple, mixer.value(MixPoint::Ripple, &context)),
                shear: gates.drive(Property::Shear, mixer.value(MixPoint::Shear, &context)),
                jitter: gates.drive(Property::Jitter, mixer.value(MixPoint::Jitter, &context)),
                phase: self.phase,
            }
        };
        self.deformation = deformation;
        self.line_energy = gates.drive(Property::Line, mixer.value(MixPoint::Line, &context));
        self.world_vertices.clear();
        self.vertices.clear();
        let pose = self.pose.with_action(flip_pitch, shake);
        for index in 0..self.wireframe.vertex_count() {
            let world = deformation.apply(self.wireframe.vertex(index, self.phase));
            self.world_vertices.push(world);
            self.vertices.push(camera.project(world, pose));
        }
        self.update_kinematics(dt);
        let depth_opacity = if self.is_showcase() {
            // Showcase models are fitted to a unit radius, so `pose.scale` is
            // the model radius in view space. Keep the planes on that fitted
            // sphere instead of following the silhouette of the current pose.
            DepthOpacity::centered(self.pose.distance, self.pose.scale, depth_fade)
        } else {
            DepthOpacity::new(self.vertices.iter().map(|vertex| vertex.depth), depth_fade)
        };
        self.current.clear();
        self.current
            .extend(self.wireframe.edges().iter().map(|edge| {
                let a = self.vertices[edge[0] as usize];
                let b = self.vertices[edge[1] as usize];
                LineGeometry {
                    start: a.screen,
                    end: b.screen,
                    start_depth_opacity: depth_opacity.factor(a.depth),
                    end_depth_opacity: depth_opacity.factor(b.depth),
                }
            }));
        let exposure = if self.motion_blur {
            if self.history.len() == MAX_HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(PoseSnapshot {
                at: frame_end,
                lines: self.current.clone(),
            });
            while self
                .history
                .front()
                .is_some_and(|snapshot| frame_end.seconds_since(snapshot.at) > HISTORY_SECONDS)
            {
                self.history.pop_front();
            }
            self.exposure_poses(exposure_start, frame_end)
        } else {
            None
        };
        for (index, current) in self.current.iter().enumerate() {
            let rgb = self.tint.edge_rgb(index, palette);
            let base_opacity = if near_opacity < computation::LINE_ENERGY_BASE {
                self.line_energy * near_opacity / computation::LINE_ENERGY_BASE
            } else {
                self.line_energy.max(near_opacity)
            };
            let alpha = self.opacity * base_opacity;
            let width = base_width * (1.0 + music.kick * 0.25);
            let alpha = alpha.clamp(0.0, 1.0);
            let start_alpha = alpha * current.start_depth_opacity;
            let end_alpha = alpha * current.end_depth_opacity;

            let Some(exposure) = &exposure else {
                let start_color = [rgb[0], rgb[1], rgb[2], start_alpha];
                let end_color = [rgb[0], rgb[1], rgb[2], end_alpha];
                out.push(if showcase {
                    LineInstance::flat(current.start, current.end, start_color, end_color, width)
                } else {
                    LineInstance::shaded(current.start, current.end, start_color, end_color, width)
                });
                continue;
            };
            let first = exposure.first[index];
            let last = exposure.last[index];
            let middle = lerp_line(first, last, 0.5);
            let distance_a = (last.start - first.start).length();
            let distance_b = (last.end - first.end).length();
            let movement = distance_a.max(distance_b);
            let fade = |distance: f32| motion_fade.factor(distance, width);
            // A fast edge spreads the same ink over more pixels, so its peak
            // opacity is lower. Even a motionless edge keeps the central body.
            let peak = alpha * fade(movement);
            // The analytic fallback stands in for that body, so it takes the
            // same peak fade and the alpha stays continuous at the sub-pixel
            // threshold.
            if movement <= MOTION_BLUR_MIN_PIXELS {
                let start_color = [rgb[0], rgb[1], rgb[2], peak * middle.start_depth_opacity];
                let end_color = [rgb[0], rgb[1], rgb[2], peak * middle.end_depth_opacity];
                out.push(if showcase {
                    LineInstance::flat(middle.start, middle.end, start_color, end_color, width)
                } else {
                    LineInstance::shaded(middle.start, middle.end, start_color, end_color, width)
                });
                continue;
            }
            let start_alpha = alpha * fade(distance_a) * middle.start_depth_opacity;
            let end_alpha = alpha * fade(distance_b) * middle.end_depth_opacity;
            let peak_x_alpha = peak * (middle.start_depth_opacity + middle.end_depth_opacity) * 0.5;

            if segment_intersection(first.start, first.end, last.start, last.end).is_some() {
                let direction = (middle.end - middle.start).normalized();
                let mut normal = if direction.length() > 0.0 {
                    direction.perpendicular()
                } else {
                    Vec2::new(0.0, 1.0)
                };
                // Keep A1 on the X1 side regardless of projected edge order.
                if (first.start - middle.start).x * normal.x
                    + (first.start - middle.start).y * normal.y
                    < 0.0
                {
                    normal = normal * -1.0;
                }
                let body_half = (width * 0.5).max(0.5);
                let l_top = middle.start + normal * body_half;
                let l_bottom = middle.start - normal * body_half;
                let r_top = middle.end + normal * body_half;
                let r_bottom = middle.end - normal * body_half;
                let x1 = (l_top + r_top) * 0.5;
                let x2 = (l_bottom + r_bottom) * 0.5;
                let clear = [rgb[0], rgb[1], rgb[2], 0.0];
                let am = [rgb[0], rgb[1], rgb[2], start_alpha];
                let bm = [rgb[0], rgb[1], rgb[2], end_alpha];
                let peak_x = [rgb[0], rgb[1], rgb[2], peak_x_alpha];

                // The body is two flat-capped quads; the shared long-edge
                // gradients are the same U -> X -> V ramps as the wings.
                out_ribbons.push(RibbonInstance::triangle(
                    [l_top, l_bottom, x2],
                    [am, am, peak_x],
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [l_top, x2, x1],
                    [am, peak_x, peak_x],
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [x1, x2, r_bottom],
                    [peak_x, peak_x, bm],
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [x1, r_bottom, r_top],
                    [peak_x, bm, bm],
                ));
                // Top wings attach to the top long edge and meet at X1. Their
                // outer-to-X edges are masked to zero by the wing shader.
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [first.start, l_top, x1],
                    [clear, am, peak_x],
                ));
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [last.end, r_top, x1],
                    [clear, bm, peak_x],
                ));
                // Bottom wings attach to the bottom long edge and meet at X2.
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [last.start, l_bottom, x2],
                    [clear, am, peak_x],
                ));
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [first.end, r_bottom, x2],
                    [clear, bm, peak_x],
                ));
                continue;
            }

            let direction = (middle.end - middle.start).normalized();
            let mut normal = if direction.length() > 0.0 {
                direction.perpendicular()
            } else {
                Vec2::new(0.0, 1.0)
            };
            // Point the body normal from the first pose toward the last pose.
            let motion = ((last.start - first.start) + (last.end - first.end)) * 0.5;
            if normal.x * motion.x + normal.y * motion.y < 0.0 {
                normal = normal * -1.0;
            }
            let half = (width * 0.5).max(0.5);
            let back_start = middle.start - normal * half;
            let back_end = middle.end - normal * half;
            let front_start = middle.start + normal * half;
            let front_end = middle.end + normal * half;
            let clear = [rgb[0], rgb[1], rgb[2], 0.0];
            let opaque_start = [rgb[0], rgb[1], rgb[2], peak * middle.start_depth_opacity];
            let opaque_end = [rgb[0], rgb[1], rgb[2], peak * middle.end_depth_opacity];

            // One continuous strip: incoming gradient, body, outgoing
            // gradient. The body is the middle band, so the boundary vertices
            // and colors are shared exactly with the gradient sections.
            out_ribbons.push(RibbonInstance::triangle(
                [first.start, first.end, back_end],
                [clear, clear, opaque_end],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [first.start, back_end, back_start],
                [clear, opaque_end, opaque_start],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [back_start, back_end, front_end],
                [opaque_start, opaque_end, opaque_end],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [back_start, front_end, front_start],
                [opaque_start, opaque_end, opaque_start],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [front_start, front_end, last.end],
                [opaque_start, opaque_end, clear],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [front_start, last.end, last.start],
                [opaque_start, clear, clear],
            ));
        }
    }

    fn update_kinematics(&mut self, dt: f32) {
        let count = self.vertices.len();
        self.previous_screen.resize(
            count,
            self.vertices
                .first()
                .map_or(Vec2::new(0.0, 0.0), |v| v.screen),
        );
        self.velocity.resize(count, Vec2::new(0.0, 0.0));
        self.acceleration.resize(count, Vec2::new(0.0, 0.0));
        let blend = 1.0 - (-dt * 10.0).exp();
        for index in 0..count {
            let current = self.vertices[index].screen;
            let previous = self.previous_screen[index];
            let old_velocity = self.velocity[index];
            let old_acceleration = self.acceleration[index];
            let (raw_velocity, raw_acceleration) = if dt > 1e-4 {
                let velocity = (current - previous) * (1.0 / dt);
                let acceleration = (velocity - old_velocity) * (1.0 / dt);
                (velocity, acceleration)
            } else {
                (old_velocity, Vec2::new(0.0, 0.0))
            };
            self.velocity[index] = old_velocity + (raw_velocity - old_velocity) * blend;
            self.acceleration[index] =
                old_acceleration + (raw_acceleration - old_acceleration) * blend;
            self.previous_screen[index] = current;
        }
    }

    fn vertex_diagnosis(&self, index: usize) -> Option<VertexDiagnosis> {
        Some(VertexDiagnosis {
            screen: self.vertices.get(index)?.screen,
            world: *self.world_vertices.get(index)?,
            velocity: *self.velocity.get(index)?,
            acceleration: *self.acceleration.get(index)?,
        })
    }

    /// The two recorded poses spanning one shutter exposure. The middle pose is
    /// reconstructed as the median of the endpoints, never sampled from history,
    /// so the gradient is centred on a straight interpolation of the exposure.
    fn exposure_poses(&self, start: AudioTime, end: AudioTime) -> Option<ExposurePoses> {
        if end <= start || self.history.len() < 2 {
            return None;
        }
        let duration = end.seconds_since(start);
        if duration <= 1e-6 {
            return None;
        }
        Some(ExposurePoses {
            first: self.pose_at(start)?,
            last: self.pose_at(end)?,
        })
    }

    fn pose_at(&self, time: AudioTime) -> Option<Vec<LineGeometry>> {
        let first = self.history.front()?;
        let last = self.history.back()?;
        if time <= first.at {
            return Some(first.lines.clone());
        }
        if time >= last.at {
            return Some(last.lines.clone());
        }
        let mut before = first;
        for snapshot in self.history.iter().skip(1) {
            if snapshot.at >= time {
                let span = snapshot.at.seconds_since(before.at);
                let fraction = if span > 1e-9 {
                    (time.seconds_since(before.at) / span).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                };
                return Some(
                    before
                        .lines
                        .iter()
                        .zip(&snapshot.lines)
                        .map(|(&from, &to)| lerp_line(from, to, fraction))
                        .collect(),
                );
            }
            before = snapshot;
        }
        Some(last.lines.clone())
    }
}

struct ExposurePoses {
    first: Vec<LineGeometry>,
    last: Vec<LineGeometry>,
}

/// A constant-width quad stroke with independent opacity at each end. Used for
/// the short, faded-in intersection cores rather than the full median edges.
/// A single beveled V stroke through X. The two arms share one join edge, so the
/// intersection opacity is composited once instead of twice.
fn segment_intersection(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> Option<Vec2> {
    let r = a1 - a0;
    let s = b1 - b0;
    let denominator = r.x * s.y - r.y * s.x;
    if denominator.abs() < 1e-6 {
        return None;
    }
    let offset = b0 - a0;
    let t = (offset.x * s.y - offset.y * s.x) / denominator;
    let u = (offset.x * r.y - offset.y * r.x) / denominator;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(a0 + r * t)
    } else {
        None
    }
}

fn lerp_line(from: LineGeometry, to: LineGeometry, fraction: f32) -> LineGeometry {
    LineGeometry {
        start: from.start.lerp(to.start, fraction),
        end: from.end.lerp(to.end, fraction),
        start_depth_opacity: from.start_depth_opacity
            + (to.start_depth_opacity - from.start_depth_opacity) * fraction,
        end_depth_opacity: from.end_depth_opacity
            + (to.end_depth_opacity - from.end_depth_opacity) * fraction,
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Style {
    axis: Vec3,
    twist_sign: f32,
    shear_sign: f32,
    deform_bias: f32,
    zoom_bias: f32,
    phase_rate: f32,
    jitter_bias: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            axis: Vec3::new(0.55, 0.25, 0.4).normalized(),
            twist_sign: 1.0,
            shear_sign: 1.0,
            deform_bias: 0.55,
            zoom_bias: 1.0,
            phase_rate: 1.0,
            jitter_bias: 0.5,
        }
    }
}

impl Style {
    fn random(rng: &mut Rng) -> Self {
        let axes = [
            Vec3::new(1.0, 0.2, 0.1),
            Vec3::new(0.15, 1.0, 0.2),
            Vec3::new(0.1, 0.25, 1.0),
            Vec3::new(0.7, 0.6, 0.35),
            Vec3::new(-0.6, 0.8, 0.3),
        ];
        Self {
            axis: axes[rng.range(axes.len())].normalized(),
            twist_sign: if rng.chance(0.5) { 1.0 } else { -1.0 },
            shear_sign: if rng.chance(0.5) { 1.0 } else { -1.0 },
            deform_bias: 0.35 + rng.unit() * 0.95,
            zoom_bias: 0.9 + rng.unit() * 0.25,
            phase_rate: 0.7 + rng.unit() * 0.7,
            jitter_bias: 0.2 + rng.unit() * 1.1,
        }
    }

    fn approach(&mut self, target: Self, dt: f32, rate: f32) {
        let blend = 1.0 - (-rate * dt).exp();
        // Do not renormalize here: opposite target axes must pass through zero
        // smoothly instead of snapping through a singularity.
        self.axis = self.axis + (target.axis - self.axis) * blend;
        self.twist_sign += (target.twist_sign - self.twist_sign) * blend;
        self.shear_sign += (target.shear_sign - self.shear_sign) * blend;
        self.deform_bias += (target.deform_bias - self.deform_bias) * blend;
        self.zoom_bias += (target.zoom_bias - self.zoom_bias) * blend;
        self.phase_rate += (target.phase_rate - self.phase_rate) * blend;
        self.jitter_bias += (target.jitter_bias - self.jitter_bias) * blend;
    }
}

fn style_value(style: &Style, input: StyleInput) -> f32 {
    match input {
        StyleInput::AxisX => style.axis.x,
        StyleInput::AxisY => style.axis.y,
        StyleInput::AxisZ => style.axis.z,
        StyleInput::PhaseRate => style.phase_rate,
        StyleInput::TwistSign => style.twist_sign,
        StyleInput::ShearSign => style.shear_sign,
        StyleInput::DeformBias => style.deform_bias,
        StyleInput::ZoomBias => style.zoom_bias,
        StyleInput::JitterBias => style.jitter_bias,
    }
}

fn set_style_value(style: &mut Style, input: StyleInput, value: f32) {
    match input {
        StyleInput::AxisX => style.axis.x = value,
        StyleInput::AxisY => style.axis.y = value,
        StyleInput::AxisZ => style.axis.z = value,
        StyleInput::PhaseRate => style.phase_rate = value,
        StyleInput::TwistSign => style.twist_sign = value,
        StyleInput::ShearSign => style.shear_sign = value,
        StyleInput::DeformBias => style.deform_bias = value,
        StyleInput::ZoomBias => style.zoom_bias = value,
        StyleInput::JitterBias => style.jitter_bias = value,
    }
}

/// The palette assignment of one spawned figure: which clean vivid theme color
/// each edge uses. The assignment is frozen at spawn, so the figure keeps its
/// colors until it leaves the scene.
#[derive(Clone, Copy, Debug)]
struct FigureTint {
    primary: usize,
    secondary: Option<usize>,
}

impl FigureTint {
    /// `accent` is the accent band value at spawn; it shifts the chosen theme
    /// color without making it follow the spectrum afterwards.
    fn spawn(kind: FigureKind, accent: f32, rng: &mut Rng) -> Self {
        let offset = accent_index(accent);
        let primary = (offset + rng.range(Palette::VIVID_COUNT)) % Palette::VIVID_COUNT;
        let secondary = kind
            .uses_two_accents()
            .then(|| (primary + 1 + rng.range(Palette::VIVID_COUNT - 1)) % Palette::VIVID_COUNT);
        Self { primary, secondary }
    }

    /// A model is one coherent object, so every edge shares the accent color.
    fn solid(accent: f32) -> Self {
        Self {
            primary: accent_index(accent),
            secondary: None,
        }
    }

    /// RGB for one edge. The palette is sampled live so a theme crossfade
    /// reaches figures that are already on screen.
    fn edge_rgb(self, edge: usize, palette: &Palette) -> [f32; 3] {
        let index = match self.secondary {
            Some(secondary) if edge % 2 == 1 => secondary,
            _ => self.primary,
        };
        let color = palette.vivid(index);
        [
            color.components[0],
            color.components[1],
            color.components[2],
        ]
    }
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn accent_index(accent: f32) -> usize {
    (accent.clamp(0.0, 1.0) * (Palette::VIVID_COUNT - 1) as f32).round() as usize
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn unit(&mut self) -> f32 {
        ((self.next() >> 40) as f32) / ((1u64 << 24) as f32)
    }

    fn chance(&mut self, probability: f32) -> bool {
        self.unit() < probability
    }

    fn range(&mut self, end: usize) -> usize {
        assert!(end > 0);
        ((self.unit() * end as f32) as usize).min(end - 1)
    }

    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_defaults_describe_the_onset_bank() {
        let detectors = DetectorSettings::default();
        let band = detectors.band_onset[0];
        assert_eq!(band.abl_gain, 1.0);
        assert_eq!(band.abl_curve, Curve::SCurve);
        assert_eq!(band.abl_amount, 2.5);
        assert!(detectors
            .band_onset
            .iter()
            .all(|candidate| *candidate == band));
        assert_eq!(detectors.excess.background_attack, 0.320);
        assert_eq!(detectors.excess.background_release, 0.200);
        assert_eq!(detectors.excess.floor, 0.010);
        assert_eq!(detectors.excess.cap, 1.0);
        assert!(detectors.kick_contains(0));
        assert!(detectors.kick_contains(15));
        assert!(detectors.snare_contains(4));
        assert!(detectors.snare_contains(15));
        assert!(!detectors.snare_contains(1));
        assert_eq!(detectors.kick_onset_min_interval, 0.09514833);
        assert_eq!(detectors.kick_onset_pre_gain, 0.49823883);
        assert_eq!(detectors.kick_onset_suppression, 0.5);
        assert_eq!(detectors.snare_onset_attack, 0.001);
        assert_eq!(detectors.snare_onset_pre_gain, 1.0054638);
        assert_eq!(detectors.snare_onset_suppression, 0.32999998);
        assert_eq!(detectors.drop_gap_window, 1.0);
        assert_eq!(detectors.drop_gap_threshold, -0.3);
        assert_eq!(detectors.drop_gap_gain, 1.0);
        assert_eq!(detectors.drop_onset_pre_gain, 1.0054638);
        assert_eq!(detectors.drop_onset_attack, 0.024582442);
        assert_eq!(detectors.drop_onset_min_threshold, 0.125);
        assert_eq!(detectors.drop_onset_suppression, 0.5);
        assert_eq!(detectors.drop_kernels.low[0], 0.9628592);
        assert_eq!(detectors.drop_kernels.low[4], 0.80759937);
        assert_eq!(detectors.drop_kernels.high[4], 0.2293353);
        assert_eq!(detectors.drop_kernels.onset[0], 0.44955054);
        assert!(detectors.clone().validate().is_ok());

        let mut broken = detectors;
        broken.band_onset[7].abl_gain = 0.0;
        assert!(broken.validate().is_err());
    }

    #[test]
    fn composite_onset_threshold_slider_stops_at_a_quarter() {
        let mut visualizer = Visualizer::new();
        for detector in [Detector::Kick, Detector::Snare, Detector::Drop] {
            let threshold = Parameter::OnsetFlux(detector, FluxSetting::MinThreshold);
            assert_eq!(threshold.range(), (0.0, 0.25));
            visualizer.set_parameter(threshold, 100.0);
            assert_eq!(visualizer.parameter_value(threshold), 0.25);
        }
    }

    #[test]
    fn suppression_slider_spans_zero_to_double_subtraction() {
        let mut visualizer = Visualizer::new();
        for detector in [Detector::Kick, Detector::Snare, Detector::Drop] {
            let suppression = Parameter::OnsetSuppression(detector);
            assert_eq!(suppression.label(), "ONS SUPPRESS");
            assert_eq!(
                suppression.range(),
                (FLUX_SUPPRESSION_MIN, FLUX_SUPPRESSION_MAX)
            );
            visualizer.set_parameter(suppression, 100.0);
            assert_eq!(
                visualizer.parameter_value(suppression),
                FLUX_SUPPRESSION_MAX
            );
            visualizer.set_parameter(suppression, -1.0);
            assert_eq!(
                visualizer.parameter_value(suppression),
                FLUX_SUPPRESSION_MIN
            );
        }
    }

    #[test]
    fn default_kernels_select_their_bands() {
        let detectors = DetectorSettings::default();
        assert!(detectors.kick_contains(0));
        assert!(detectors.kick_contains(15));
        assert!(detectors.snare_contains(4));
        assert!(detectors.snare_contains(15));
        assert!(!detectors.snare_contains(1));
    }

    #[test]
    fn all_bands_start_with_adaptive_references() {
        let visualizer = Visualizer::new();
        for band in 0..BAND_COUNT {
            let shape = visualizer.band_shape(band);
            assert_eq!(shape.tracking, ReferenceTracking::Adaptive, "band {band}");
        }
        assert_eq!(
            visualizer.input_shape(Input::Power).unwrap().tracking,
            ReferenceTracking::Manual
        );
    }

    #[test]
    fn depth_fade_adjusts_in_five_percent_steps_and_stays_normalized() {
        let fade = DepthFade::default();
        assert_eq!(fade.percent(), 50);
        assert_eq!(fade.adjusted(Adjustment::Increase).percent(), 55);
        assert_eq!(fade.adjusted(Adjustment::Decrease).percent(), 45);

        let mut lowest = fade;
        for _ in 0..30 {
            lowest = lowest.adjusted(Adjustment::Decrease);
        }
        assert_eq!(lowest.strength(), 0.0);

        let mut highest = fade;
        for _ in 0..30 {
            highest = highest.adjusted(Adjustment::Increase);
        }
        assert_eq!(highest.strength(), 1.0);

        assert!(DepthFade::try_from(-0.01).is_err());
        assert!(DepthFade::try_from(1.01).is_err());
        assert!(DepthFade::try_from(f32::NAN).is_err());
    }

    #[test]
    fn near_opacity_uses_five_percent_then_one_percent_steps() {
        let opacity = NearOpacity::default();
        assert_eq!(opacity.percent(), 60);
        assert_eq!(opacity.adjusted(Adjustment::Increase).percent(), 65);
        assert_eq!(opacity.adjusted(Adjustment::Decrease).percent(), 55);

        let mut highest = opacity;
        for _ in 0..30 {
            highest = highest.adjusted(Adjustment::Increase);
        }
        assert_eq!(highest.value(), 1.0);

        let mut lowest = opacity;
        for _ in 0..20 {
            lowest = lowest.adjusted(Adjustment::Decrease);
        }
        assert_eq!(lowest.percent(), 1);

        let ten = NearOpacity::try_from(0.10).expect("ten percent is valid");
        assert_eq!(ten.adjusted(Adjustment::Decrease).percent(), 9);
        assert_eq!(ten.adjusted(Adjustment::Increase).percent(), 15);
        let nine = ten.adjusted(Adjustment::Decrease);
        assert_eq!(nine.adjusted(Adjustment::Increase).percent(), 10);

        assert!(NearOpacity::try_from(0.005).is_err());
        assert!(NearOpacity::try_from(1.01).is_err());
        assert!(NearOpacity::try_from(f32::NAN).is_err());
    }

    #[test]
    fn detector_settings_reach_the_debug_snapshot_and_parameters() {
        let mut visualizer = Visualizer::new();
        let settings = DetectorSettings {
            kick_onset_min_interval: 0.05,
            snare_onset_attack: 0.01,
            drop_gap_window: 0.6,
            drop_gap_gain: 2.0,
            drop_gap_threshold: -0.1,
            drop_onset_attack: 0.015,
            ..DetectorSettings::default()
        };
        visualizer.set_detector_settings(settings.clone());
        let snapshot = visualizer.debug_snapshot();
        assert!(
            (visualizer.parameter_value(Parameter::OnsetFlux(
                Detector::Kick,
                FluxSetting::MinInterval
            )) - 0.05)
                .abs()
                < 1e-6
        );
        assert!(
            (visualizer
                .parameter_value(Parameter::OnsetFlux(Detector::Snare, FluxSetting::Attack))
                - 0.01)
                .abs()
                < 1e-6
        );
        assert!((visualizer.parameter_value(Parameter::DropGapWindow) - 0.6).abs() < 1e-6);
        assert!((visualizer.parameter_value(Parameter::DropGapGain) - 2.0).abs() < 1e-6);
        assert!((visualizer.parameter_value(Parameter::DropGapThreshold) + 0.1).abs() < 1e-6);
        assert!(
            (visualizer.parameter_value(Parameter::OnsetFlux(Detector::Drop, FluxSetting::Attack))
                - 0.015)
                .abs()
                < 1e-6
        );
        assert!(visualizer.detector_settings().kick_contains(0));
        assert!(visualizer.detector_settings().kick_contains(15));
        assert!(visualizer.detector_settings().snare_contains(4));
        assert!(!visualizer.detector_settings().snare_contains(1));
        assert_eq!(snapshot.kick_hits, 0);
        assert_eq!(snapshot.snare_hits, 0);
        assert_eq!(snapshot.drop_hits, 0);

        visualizer.set_parameter(
            Parameter::OnsetFlux(Detector::Kick, FluxSetting::Sensitivity),
            3.0,
        );
        assert_eq!(
            visualizer.parameter_value(Parameter::OnsetFlux(
                Detector::Kick,
                FluxSetting::Sensitivity
            )),
            3.0
        );
    }

    #[test]
    fn abl_and_excess_sliders_clamp_and_format() {
        let mut visualizer = Visualizer::new();
        let gain = Parameter::BandAblGain(4);
        let amount = Parameter::BandAblAmount(4);
        visualizer.set_parameter(gain, 100.0);
        assert_eq!(visualizer.parameter_value(gain), ABL_GAIN_MAX);
        assert_eq!(
            visualizer.parameter_value(Parameter::BandAblGain(3)),
            1.0,
            "editing one band must not touch another"
        );
        visualizer.set_parameter(gain, 0.0);
        assert_eq!(visualizer.parameter_value(gain), ABL_GAIN_MIN);
        visualizer.set_parameter(amount, 100.0);
        assert_eq!(visualizer.parameter_value(amount), Curve::MAX_AMOUNT);
        let curve = Parameter::BandAblCurve(4);
        visualizer.set_parameter(curve, 3.0);
        assert_eq!(visualizer.parameter_value(curve), 3.0);
        assert_eq!(curve.format(3.0), Curve::from_index(3).name());
        assert_eq!(gain.format(2.0), "2.00");

        let attack = Parameter::ExcessAttack;
        visualizer.set_parameter(attack, 100.0);
        assert_eq!(visualizer.parameter_value(attack), EXCESS_ATTACK_MAX);
        let floor = Parameter::ExcessFloor;
        visualizer.set_parameter(floor, 100.0);
        assert_eq!(visualizer.parameter_value(floor), EXCESS_FLOOR_MAX);
        let cap = Parameter::ExcessCap;
        visualizer.set_parameter(cap, 100.0);
        assert_eq!(visualizer.parameter_value(cap), EXCESS_CAP_MAX);
        assert_eq!(floor.format(0.0123), "0.012");
        assert_eq!(
            Parameter::OnsetFlux(Detector::Kick, FluxSetting::MinThreshold).range(),
            (FLUX_THRESHOLD_MIN, ONSET_THRESHOLD_MAX)
        );
    }

    #[test]
    fn temporal_pattern_clear_and_vertex_edits_persist() {
        let mut visualizer = Visualizer::new();
        visualizer.clear_pattern(Detector::Kick);
        assert_eq!(
            visualizer.detector_settings().kick_pattern,
            TemporalPattern::kick_initial()
        );
        visualizer.set_pattern_vertex(
            Detector::Kick,
            PatternKernel::Main,
            PatternLayer::Inner,
            0,
            [0.2, 0.1],
        );
        let vertex = visualizer.detector_settings().kick_pattern.inner[0];
        assert!((vertex[0] - 0.2).abs() < 1e-6);
        assert!((vertex[1] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn drop_lp_sliders_control_the_kernel_outputs() {
        let mut visualizer = Visualizer::new();
        let low = Parameter::DropLowLp;
        let high = Parameter::DropHighLp;
        assert_eq!(low.range(), (DROP_LP_MIN, DROP_LP_MAX));
        assert_eq!(low.format(0.05), "50ms");
        visualizer.set_parameter(low, 0.05);
        visualizer.set_parameter(high, 0.2);
        assert!((visualizer.parameter_value(low) - 0.05).abs() < 1e-6);
        assert!((visualizer.parameter_value(high) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn kernel_area_slider_limits_the_temporal_template() {
        let mut visualizer = Visualizer::new();
        let area = Parameter::PatternArea(Detector::Kick);
        let snare_area = visualizer.parameter_value(Parameter::PatternArea(Detector::Snare));
        visualizer.set_parameter(area, 0.5);
        assert!((visualizer.parameter_value(area) - 0.5).abs() < 1e-6);
        assert_eq!(
            visualizer.parameter_value(Parameter::PatternArea(Detector::Snare)),
            snare_area,
            "the area is per detector"
        );
        assert_eq!(area.format(0.5), "50%");
        assert_eq!(area.range(), (0.1, 1.0));
    }

    #[test]
    fn onset_pre_gain_slider_trims_each_detector() {
        let mut visualizer = Visualizer::new();
        let kick = Parameter::OnsetFlux(Detector::Kick, FluxSetting::PreGain);
        let snare = Parameter::OnsetFlux(Detector::Snare, FluxSetting::PreGain);
        let snare_gain = visualizer.parameter_value(snare);
        visualizer.set_parameter(kick, 0.1);
        assert!((visualizer.parameter_value(kick) - 0.1).abs() < 1e-6);
        assert_eq!(
            visualizer.parameter_value(snare),
            snare_gain,
            "the trim is per detector"
        );
        assert_eq!(
            Parameter::OnsetFlux(Detector::Kick, FluxSetting::PreGain).range(),
            (FLUX_PRE_GAIN_MIN, FLUX_PRE_GAIN_MAX)
        );
        assert_eq!(
            Parameter::OnsetFlux(Detector::Kick, FluxSetting::PreGain).format(0.25),
            "0.25"
        );
    }

    #[test]
    fn raw_band_shape_parameters_target_the_selected_band() {
        let mut visualizer = Visualizer::new();
        let target = EnvelopeTarget::BandIndex(7);
        visualizer.set_parameter(Parameter::InputLow(target), 0.3);
        visualizer.set_parameter(Parameter::InputHigh(target), 0.7);
        visualizer.set_parameter(Parameter::InputCurve(target), 3.0);
        visualizer.set_parameter(Parameter::InputAmount(target), 2.0);
        let shape = visualizer.band_shape(7);
        assert_eq!(shape.low, 0.3);
        assert_eq!(shape.high, 0.7);
        assert_eq!(shape.curve.index(), 3);
        assert_eq!(shape.amount, 2.0);
        assert!(shape.is_adaptive());
        visualizer.toggle_adaptive(target);
        assert!(!visualizer.band_shape(7).is_adaptive());
        let other = visualizer.band_shape(6);
        assert_eq!(other.low, 0.0);
        assert_eq!(other.high, 1.0);
    }

    #[test]
    fn a_step_in_spectrum_energy_moves_lines_without_jumping_out_of_view() {
        let mut visualizer = Visualizer::new();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut previous: Vec<[f32; 2]> = Vec::new();
        for step in 0..120 {
            let level = (step as f32 / 60.0).min(1.0);
            visualizer.debug_music([level; BAND_COUNT], level);
            visualizer.update(
                frame,
                ShutterFrames::try_from(1.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            let current: Vec<[f32; 2]> = visualizer
                .scene
                .current
                .current
                .iter()
                .map(|line| [line.end.x, line.end.y])
                .collect();
            if !previous.is_empty() {
                let moved = current
                    .iter()
                    .zip(&previous)
                    .map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]))
                    .fold(0.0_f32, f32::max);
                assert!(
                    moved < 180.0,
                    "the figure jumped {moved} pixels in one frame"
                );
            }
            previous = current;
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        for line in visualizer.frame().lines {
            for value in &line.endpoints {
                assert!(value.is_finite());
            }
            assert!(line.endpoints[0] >= -1280.0 && line.endpoints[0] <= 2560.0);
            assert!(line.endpoints[1] >= -440.0 && line.endpoints[1] <= 880.0);
        }
    }

    #[test]
    fn kick_snare_and_drop_actions_move_the_figure_state() {
        let mut visualizer = Visualizer::new();
        let settings = DetectorSettings {
            snare_flip_every: 2,
            ..DetectorSettings::default()
        };
        visualizer.set_detector_settings(settings.clone());

        visualizer.handle_action(Action::Kick(0.5), 0.0);
        assert!(visualizer.kick >= 0.5);
        visualizer
            .quake_x
            .advance(0.01, settings.quake_stiffness, settings.quake_damping);
        assert!(
            visualizer.quake_x.value() < 0.0,
            "the first kick shakes left"
        );

        visualizer.handle_action(Action::Snare, 0.0);
        assert_eq!(visualizer.flip_target, 0.0);
        visualizer.handle_action(Action::Snare, 0.0);
        assert!((visualizer.flip_target - TAU * 0.5).abs() < 1e-5);

        let before = visualizer.scene_name().to_owned();
        visualizer.handle_action(Action::Drop(1.0), 0.0);
        assert_ne!(
            visualizer.scene_name(),
            before,
            "a drop always changes figure"
        );
        visualizer
            .quake_y
            .advance(0.01, settings.quake_stiffness, settings.quake_damping);
        assert!(visualizer.quake_y.value() > 0.0, "the drop shakes downward");
    }

    #[test]
    fn low_band_bursts_trigger_the_kick_and_quake() {
        let mut visualizer = Visualizer::new();
        // Isolate the kick kernel so a broadband click cannot choke it
        // through the snare detector.
        visualizer.set_detector_settings(DetectorSettings {
            snare_pattern: TemporalPattern {
                inner: vec![[0.0, 0.999], [1.0, 0.999], [1.0, 1.0], [0.0, 1.0]],
                outer: vec![[0.0, 0.999], [1.0, 0.999], [1.0, 1.0], [0.0, 1.0]],
                suppression_inner: vec![[0.0, 0.999], [1.0, 0.999], [1.0, 1.0], [0.0, 1.0]],
                suppression_outer: vec![[0.0, 0.999], [1.0, 0.999], [1.0, 1.0], [0.0, 1.0]],
                effective: 1.0,
            },
            ..DetectorSettings::default()
        });
        visualizer.set_samplerate(48_000);
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut at = AudioTime::ZERO;
        let mut kick_seen = false;
        let mut quake_seen = false;
        for frame in 0..360 {
            let input: Vec<[f32; 2]> = (0..800)
                .map(|index| {
                    let sample = frame * 800 + index;
                    let time = sample as f32 / 48_000.0;
                    let block = sample % 9_600;
                    let envelope = if block < 3_000 {
                        1.0 - block as f32 / 3_000.0
                    } else {
                        0.0
                    };
                    // Spread the burst over the five kick bands so the
                    // composite kick kernel sees a real low transient.
                    let bass = [40.0, 50.0, 65.0, 85.0, 110.0]
                        .iter()
                        .map(|frequency| (std::f32::consts::TAU * frequency * time).sin())
                        .sum::<f32>()
                        / 5.0
                        * envelope;
                    [bass; 2]
                })
                .collect();
            visualizer.consume_buf(&input, 1.0);
            let end = at.shifted(800.0 / 48_000.0);
            visualizer.update(
                AudioInterval::new(at, end),
                ShutterFrames::try_from(2.0).unwrap(),
                800.0 / 48_000.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            at = end;
            kick_seen |= visualizer.kick > 0.0;
            quake_seen |=
                visualizer.debug_snapshot().properties[Property::Quake.index()].abs() > 0.0;
        }
        assert!(
            kick_seen,
            "the kick kernel onset must reach the kick envelope"
        );
        assert!(quake_seen, "a kick must inject a quake impulse");
        assert!(visualizer.debug_snapshot().kick_hits > 0);
    }

    #[test]
    fn a_confident_gap_and_composite_onset_confirm_a_drop() {
        let mut visualizer = Visualizer::new();
        let settings = DetectorSettings {
            drop_kernels: DropKernels {
                low: std::array::from_fn(|band| if band < 4 { 1.0 } else { 0.0 }),
                high: std::array::from_fn(|band| if band >= 4 { 1.0 } else { 0.0 }),
                onset: std::array::from_fn(|band| if band < 4 { 1.0 } else { 0.0 }),
            },
            drop_gap_window: 0.1,
            drop_gap_threshold: 0.1,
            drop_low_lp: 0.0,
            drop_high_lp: 0.0,
            ..DetectorSettings::default()
        };
        visualizer.set_detector_settings(settings.clone());
        let hop = 1.0 / 86.0;
        let quiet_bands = std::array::from_fn(|band| if band < 4 { 0.0 } else { 0.8 });
        let quiet = Spectrum {
            bands: quiet_bands,
            raw_bands: quiet_bands,
            ..Spectrum::default()
        };
        for _ in 0..12 {
            visualizer.observe(quiet, hop);
        }
        let armed = visualizer.debug_snapshot();
        assert!(armed.drop_onset.armed, "the missing bass arms the drop");
        assert!(armed.drop_status.high > armed.drop_status.low);
        assert!((armed.drop_status.diff - 0.8).abs() < 1e-5);
        assert!(armed.drop_status.confidence >= settings.drop_gap_threshold);

        let mut burst = quiet;
        burst.band_excess = [1.0; BAND_COUNT];
        for band in 0..4 {
            burst.bands[band] = 1.0;
            burst.raw_bands[band] = 1.0;
        }
        visualizer.observe(burst, hop);
        assert_eq!(visualizer.debug_snapshot().drop_hits, 0);
        visualizer.observe(quiet, hop);
        let confirmed = visualizer.debug_snapshot();
        assert_eq!(
            confirmed.drop_hits, 1,
            "the composite onset confirms the confident gap"
        );
        assert_eq!(
            confirmed.drop_status.confidence, 0.0,
            "confirmation restarts the gap window"
        );
    }

    #[test]
    fn longer_shutter_keeps_more_of_the_recorded_motion_path() {
        fn trail(shutter: f32) -> (usize, f32) {
            let mut visualizer = Visualizer::new();
            let palette = Palette::from_theme(crate::theme::ThemeId::default());
            let mut frame =
                AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
            for _ in 0..14 {
                visualizer.debug_music([0.8; BAND_COUNT], 0.8);
                visualizer.update(
                    frame,
                    ShutterFrames::try_from(shutter).unwrap(),
                    1.0 / 60.0,
                    &palette,
                    (1280.0, 440.0),
                    1.0,
                    1.0,
                );
                frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
            }
            let exposure_start = frame.start().shifted(-f64::from(shutter) / 60.0);
            let exposure = visualizer
                .scene
                .current
                .exposure_poses(exposure_start, frame.start())
                .expect("exposure poses");
            let movement = exposure
                .first
                .iter()
                .zip(&exposure.last)
                .map(|(first, last)| {
                    (last.start - first.start)
                        .length()
                        .max((last.end - first.end).length())
                })
                .sum();
            (visualizer.frame().ribbons.len(), movement)
        }
        let (short_count, short_movement) = trail(1.0);
        let (long_count, long_movement) = trail(8.0);
        assert!(short_count > 0);
        assert!(long_count > 0);
        assert!(long_movement > short_movement);
    }

    #[test]
    fn near_opacity_floor_scales_below_the_line_energy_base() {
        fn peak_alpha(near_opacity: NearOpacity) -> f32 {
            let mut visualizer = Visualizer::new();
            visualizer.set_near_opacity(near_opacity);
            visualizer.debug_music([0.0; BAND_COUNT], 0.0);
            let palette = Palette::from_theme(crate::theme::ThemeId::default());
            let frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
            visualizer.update(
                frame,
                ShutterFrames::try_from(1.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            visualizer
                .frame()
                .lines
                .iter()
                .flat_map(|line| [line.start_color[3], line.end_color[3]])
                .fold(0.0, f32::max)
        }

        let normal = peak_alpha(NearOpacity::default());
        let low = peak_alpha(NearOpacity::try_from(0.01).unwrap());
        assert!(normal > 0.0);
        assert!(
            low < normal * 0.1,
            "a 1% near-opacity floor must scale the line opacity below the 60% line base"
        );
    }

    #[test]
    fn edge_opacity_fades_with_projected_depth() {
        let mut visualizer = Visualizer::new();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        visualizer.debug_music([0.0; BAND_COUNT], 0.0);
        let frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        visualizer.update(
            frame,
            ShutterFrames::try_from(1.0).unwrap(),
            1.0 / 60.0,
            &palette,
            (1280.0, 440.0),
            1.0,
            1.0,
        );
        let frame = visualizer.frame();
        assert_eq!(frame.lines.len(), 12, "the first frame is the cube edges");
        let alphas: Vec<f32> = frame
            .lines
            .iter()
            .flat_map(|line| [line.start_color[3], line.end_color[3]])
            .collect();
        let min = alphas.iter().copied().fold(f32::INFINITY, f32::min);
        let max = alphas.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (max / min - 2.0).abs() < 1e-4,
            "the farthest endpoint must keep half the nearest opacity: {min}..{max}"
        );
        assert!(
            frame
                .lines
                .iter()
                .any(|line| (line.start_color[3] - line.end_color[3]).abs() > 0.01),
            "an edge must interpolate between its endpoint depths"
        );

        visualizer.set_depth_fade(DepthFade::try_from(0.0).unwrap());
        let next = AudioInterval::new(
            AudioTime::ZERO.shifted(1.0 / 60.0),
            AudioTime::ZERO.shifted(2.0 / 60.0),
        );
        visualizer.update(
            next,
            ShutterFrames::try_from(1.0).unwrap(),
            1.0 / 60.0,
            &palette,
            (1280.0, 440.0),
            1.0,
            1.0,
        );
        for edge in &visualizer.scene.current.current {
            assert_eq!(edge.start_depth_opacity, 1.0);
            assert_eq!(edge.end_depth_opacity, 1.0);
        }
    }

    #[test]
    fn the_tesseract_shares_one_edge_color_and_the_torus_alternates_two() {
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut rng = Rng::new(17);
        let tesseract = FigureTint::spawn(FigureKind::Tesseract, 0.8, &mut rng);
        let expected = tesseract.edge_rgb(0, &palette);
        assert!(
            palette.vivid.iter().any(|color| {
                let [red, green, blue, _] = color.components;
                [red, green, blue] == expected
            }),
            "figure colors must be clean theme colors"
        );
        for edge in 0..Wireframe::build(FigureKind::Tesseract).edges().len() {
            assert_eq!(tesseract.edge_rgb(edge, &palette), expected);
        }
        let torus = FigureTint::spawn(FigureKind::Torus, 0.8, &mut rng);
        assert_ne!(torus.edge_rgb(0, &palette), torus.edge_rgb(1, &palette));
    }

    #[test]
    fn a_spawned_figure_keeps_its_edge_colors_while_the_music_changes() {
        let mut visualizer = Visualizer::new();
        visualizer.scene.current.source = FigureSource::Procedural(FigureKind::Tesseract);
        visualizer.scene.current.wireframe = Wireframe::build(FigureKind::Tesseract);
        visualizer.scene.current.reset_temporal();
        visualizer.scene.current.tint =
            FigureTint::spawn(FigureKind::Tesseract, 0.8, &mut visualizer.rng);
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut expected: Option<[f32; 3]> = None;
        for step in 0..30 {
            let level = step as f32 / 29.0;
            visualizer.debug_music([level; BAND_COUNT], level);
            visualizer.kick = level;
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
            let frame = visualizer.frame();
            let mut rgbs: Vec<[f32; 3]> = Vec::new();
            for line in frame.lines {
                for color in [line.start_color, line.end_color] {
                    if color[3] > 0.0 {
                        rgbs.push([color[0], color[1], color[2]]);
                    }
                }
            }
            for ribbon in frame.ribbons {
                for color in [ribbon.start_color, ribbon.end_color, ribbon.third_color] {
                    if color[3] > 0.0 {
                        rgbs.push([color[0], color[1], color[2]]);
                    }
                }
            }
            assert!(!rgbs.is_empty());
            let first = *expected.get_or_insert(rgbs[0]);
            for rgb in rgbs {
                assert_eq!(rgb, first, "edge colors changed with the music");
            }
        }
    }

    #[test]
    fn crossing_segments_produce_intersection_point() {
        let crossing = segment_intersection(
            Vec2::new(-50.0, 0.0),
            Vec2::new(50.0, 0.0),
            Vec2::new(0.0, -50.0),
            Vec2::new(0.0, 50.0),
        )
        .expect("the segments cross");
        assert!(crossing.x.abs() < 1e-4);
        assert!(crossing.y.abs() < 1e-4);

        assert!(segment_intersection(
            Vec2::new(-50.0, 0.0),
            Vec2::new(50.0, 0.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(0.0, 50.0),
        )
        .is_none());
    }

    #[test]
    fn phase_rebase_preserves_tesseract_and_deformation_geometry() {
        let wireframe = Wireframe::build(FigureKind::Tesseract);
        let mut before = Deformation {
            twist: 0.3,
            bulge: 0.2,
            ripple: 0.4,
            shear: 0.1,
            jitter: 0.02,
            phase: 0.0,
        };
        let mut after = before;
        after.phase = PHASE_PERIOD;
        for index in 0..wireframe.vertex_count() {
            let a = before.apply(wireframe.vertex(index, 0.0));
            let b = after.apply(wireframe.vertex(index, PHASE_PERIOD));
            assert!(
                (a - b).length() < 0.01,
                "phase rebase jumped vertex {index}: {a:?} vs {b:?}"
            );
        }
        before.phase = TAU - 0.001;
        after.phase = TAU + 0.001;
        let a = before.apply(wireframe.vertex(0, TAU - 0.001));
        let b = after.apply(wireframe.vertex(0, TAU + 0.001));
        assert!((a - b).length() < 0.01);
    }

    #[test]
    fn tesseract_vertices_move_continuously() {
        let mut visualizer = Visualizer::new();
        visualizer.scene.current.source = FigureSource::Procedural(FigureKind::Tesseract);
        visualizer.scene.current.wireframe = Wireframe::build(FigureKind::Tesseract);
        visualizer.scene.current.reset_temporal();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut previous = None;
        let mut maximum = 0.0_f32;
        for _ in 0..240 {
            visualizer.debug_music([0.4; BAND_COUNT], 0.4);
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            let current = visualizer.scene.current.vertices[0].screen;
            if let Some(previous) = previous {
                maximum = maximum.max((current - previous).length());
            }
            previous = Some(current);
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        assert!(
            maximum < 120.0,
            "the tesseract vertex jumped {maximum} pixels in one frame"
        );
    }

    #[test]
    fn diagnosis_appears_on_a_vertex_and_follows_it_lazily() {
        let mut visualizer = Visualizer::new();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        visualizer.diagnosis.next_in = 0.0;
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut first_anchor = None;
        let mut latest_anchor = None;
        for step in 0..120 {
            visualizer.debug_music([0.7; BAND_COUNT], 0.7);
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            if step == 0 {
                // Make the follow test independent of which vertex the RNG
                // picked: the top vertices clamp the readout anchor to the
                // viewport edge and can keep it there for the whole run.
                let vertex = visualizer
                    .scene
                    .current
                    .vertices
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.screen.y.total_cmp(&b.screen.y))
                    .map(|(index, _)| index)
                    .expect("the cube has vertices");
                visualizer.diagnosis.vertex = vertex;
                visualizer.diagnosis.anchor = visualizer.diagnosis_target((1280.0, 440.0), 1.0);
                visualizer.diagnosis.refresh_timer = 0.0;
            }
            if let Some(readout) = visualizer.diagnosis() {
                assert!(
                    readout.text.starts_with("POS")
                        || readout.text.starts_with("XYZ")
                        || readout.text.starts_with("VEL")
                        || readout.text.starts_with("ACC")
                );
                let anchor = visualizer
                    .diagnosis_anchor()
                    .expect("a visible diagnosis has an anchor");
                assert!(anchor[0] >= 0.0 && anchor[0] <= 1280.0);
                assert!(anchor[1] >= 0.0 && anchor[1] <= 440.0);
                first_anchor.get_or_insert(anchor);
                latest_anchor = Some(anchor);
            }
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        let first = first_anchor.expect("diagnosis must become visible");
        let latest = latest_anchor.expect("diagnosis must stay visible");
        assert!((first[0] - latest[0]).abs() + (first[1] - latest[1]).abs() > 0.05);
        let frame = visualizer.frame();
        assert!(
            !frame.lines.is_empty(),
            "diagnosis must add line instancing"
        );
    }

    #[test]
    fn debug_snapshot_exposes_all_analysis_layers() {
        let mut visualizer = Visualizer::new();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        for _ in 0..3 {
            visualizer.debug_music([0.6; BAND_COUNT], 0.6);
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        let snapshot = visualizer.debug_snapshot();
        assert_eq!(snapshot.bands.len(), BAND_COUNT);
        assert!(snapshot.bands.iter().all(|band| band.high_hz > band.low_hz));
        assert!(snapshot.power.smooth.is_finite());
        assert!(snapshot.bass.smooth.is_finite());
        assert!(snapshot.treble.smooth.is_finite());
        assert!(snapshot.kick_onset.flash.is_finite());
        assert!(snapshot.snare_onset.flash.is_finite());
        assert!(snapshot.drop_onset.power.is_finite());
        assert!(snapshot.drop_status.low.is_finite());
        assert!(snapshot.drop_status.high.is_finite());
        assert!(snapshot.drop_status.diff.is_finite());
        assert!(snapshot.drop_status.confidence.is_finite());
        assert!(snapshot.drop_status.onset.flux.is_finite());
        assert!(snapshot.drop_status.onset.smoothed.is_finite());
        assert!(snapshot.drop_status.onset.threshold.is_finite());
        assert!(snapshot.drop_status.onset.inhibition.is_finite());
        assert!(snapshot.drop_status.onset.suppressed.is_finite());
        assert!(snapshot.kick_flux.flux.is_finite());
        assert!(snapshot.kick_flux.smoothed.is_finite());
        assert!(snapshot.kick_flux.threshold.is_finite());
        assert!(snapshot.kick_flux.inhibition.is_finite());
        assert!(snapshot.kick_flux.suppressed.is_finite());
        assert!(snapshot.snare_flux.flux.is_finite());
        assert!(snapshot.snare_flux.smoothed.is_finite());
        assert!(snapshot.snare_flux.threshold.is_finite());
        assert!(snapshot.snare_flux.inhibition.is_finite());
        assert!(snapshot.snare_flux.suppressed.is_finite());
        assert!(snapshot.bands.iter().all(|band| band.excess.is_finite()
            && band.onset.is_finite()
            && band.abdr.is_finite()
            && band.abl.is_finite()
            && band.drp.is_finite()));
        assert!(!snapshot.scene.is_empty());
        assert!(snapshot.lines + snapshot.ribbons > 0);
        assert!(snapshot.ribbons > 0);
        assert!(snapshot.properties.iter().all(|value| value.is_finite()));
        let history = visualizer.debug_history();
        assert!(history.samples().count() >= 3);
        let latest = history.latest().expect("the display recorded a sample");
        assert!(latest.properties.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn property_mute_and_solo_gate_scene_effects() {
        let mut visualizer = Visualizer::new();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut step = |visualizer: &mut Visualizer| {
            visualizer.debug_music([0.6; BAND_COUNT], 0.6);
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        };
        step(&mut visualizer);
        let live = visualizer.debug_snapshot();
        assert!(live.properties[Property::Spin.index()] > 0.0);
        assert!(live.properties[Property::Twist.index()].abs() > 0.0);

        visualizer.toggle_property_mute(Property::Spin);
        step(&mut visualizer);
        assert_eq!(
            visualizer.debug_snapshot().properties[Property::Spin.index()],
            0.0
        );
        visualizer.toggle_property_mute(Property::Spin);

        visualizer.toggle_property_solo(Property::Spin);
        step(&mut visualizer);
        let solo = visualizer.debug_snapshot();
        assert!(solo.properties[Property::Spin.index()] > 0.0);
        assert_eq!(solo.properties[Property::Twist.index()], 0.0);
        assert_eq!(solo.properties[Property::Line.index()], 1.0);
        assert_eq!(solo.properties[Property::Entrance.index()], 1.0);
        assert_eq!(solo.properties[Property::Morph.index()], 0.0);
        assert!(visualizer.property_soloed(Property::Spin));
        assert!(!visualizer.property_active(Property::Twist));

        visualizer.toggle_property_solo(Property::Spin);
        step(&mut visualizer);
        assert!(visualizer.debug_snapshot().properties[Property::Twist.index()].abs() > 0.0);
    }

    #[test]
    fn adjustable_parameters_round_trip_and_clamp() {
        let mut visualizer = Visualizer::new();
        let attack = Parameter::Attack(EnvelopeTarget::Power);
        let original = visualizer.parameter_value(attack);
        visualizer.set_parameter(attack, 9.0);
        assert_eq!(visualizer.parameter_value(attack), 9.0);
        visualizer.set_parameter(attack, 10_000.0);
        assert_eq!(visualizer.parameter_value(attack), 50.0);
        assert_eq!(attack.fraction(0.1), 0.0);
        assert_eq!(attack.fraction(50.0), 1.0);
        assert!(attack.from_fraction(0.5) < 3.0);

        let kick_gap = Parameter::OnsetFlux(Detector::Kick, FluxSetting::MinInterval);
        visualizer.set_parameter(kick_gap, 100.0);
        assert_eq!(
            visualizer.parameter_value(kick_gap),
            crate::visualizer::FLUX_INTERVAL_MAX
        );
        let gap_window = Parameter::DropGapWindow;
        visualizer.set_parameter(gap_window, 100.0);
        assert_eq!(visualizer.parameter_value(gap_window), 4.0);
        let gap_gain = Parameter::DropGapGain;
        visualizer.set_parameter(gap_gain, 100.0);
        assert_eq!(visualizer.parameter_value(gap_gain), 10.0);
        visualizer.set_parameter(gap_gain, 0.0);
        assert_eq!(visualizer.parameter_value(gap_gain), 0.1);
        let gap_threshold = Parameter::DropGapThreshold;
        visualizer.set_parameter(gap_threshold, -0.5);
        assert!((visualizer.parameter_value(gap_threshold) + 0.5).abs() < 1e-6);
        visualizer.set_parameter(gap_threshold, -100.0);
        assert_eq!(visualizer.parameter_value(gap_threshold), -1.0);
        visualizer.set_parameter(gap_threshold, 100.0);
        assert_eq!(visualizer.parameter_value(gap_threshold), 1.0);
        let sequence = Parameter::KickSequenceEvery;
        visualizer.set_parameter(sequence, 100.0);
        assert_eq!(visualizer.parameter_value(sequence), 32.0);
        let abl_gain = Parameter::BandAblGain(0);
        visualizer.set_parameter(abl_gain, 2.0);
        assert!((visualizer.parameter_value(abl_gain) - 2.0).abs() < 1e-6);

        let style = Parameter::Style(StyleInput::PhaseRate);
        visualizer.set_parameter(style, 2.25);
        assert!((visualizer.parameter_value(style) - 2.25).abs() < 1e-6);
        assert!((visualizer.parameter_value(attack) - original).abs() > 0.0);

        let curve = Parameter::InputCurve(EnvelopeTarget::Power);
        visualizer.set_parameter(curve, 3.0);
        assert_eq!(visualizer.parameter_value(curve), 3.0);
        let low = Parameter::InputLow(EnvelopeTarget::Power);
        let high = Parameter::InputHigh(EnvelopeTarget::Power);
        visualizer.set_parameter(low, 0.8);
        visualizer.set_parameter(high, 0.2);
        assert!(visualizer.parameter_value(low) < visualizer.parameter_value(high));
        let amount = Parameter::InputAmount(EnvelopeTarget::Power);
        visualizer.set_parameter(amount, 100.0);
        assert_eq!(visualizer.parameter_value(amount), 8.0);
    }

    #[test]
    fn default_tuning_is_valid() {
        VisualizerTuning::default()
            .validate()
            .expect("the built-in visualizer defaults stay inside every parameter range");
    }

    #[test]
    fn inspector_tuning_survives_a_config_round_trip() {
        let mut visualizer = Visualizer::new();
        visualizer.set_parameter(Parameter::KickDecay, 7.5);
        visualizer.set_parameter(Parameter::Gain(Gain::SpinBand), 3.25);
        visualizer.set_parameter(Parameter::BandSource(BandComponent::Spin), 9.0);
        visualizer.set_parameter(Parameter::Attack(EnvelopeTarget::Power), 22.0);
        visualizer.set_parameter(Parameter::Release(EnvelopeTarget::Bass), 8.0);
        visualizer.set_parameter(Parameter::InputAmount(EnvelopeTarget::Power), 3.0);
        visualizer.set_parameter(Parameter::Style(StyleInput::DeformBias), 1.25);
        visualizer.toggle_adaptive(EnvelopeTarget::Power);
        visualizer.toggle_property_mute(Property::Twist);
        visualizer.toggle_property_solo(Property::Spin);
        let base = Contribution::Fixed(MixPoint::PulseEnergy, 3);
        visualizer.set_parameter(Parameter::Constant(base), 0.4);
        let extra = visualizer.add_extra(MixPoint::SpinSpeed);
        visualizer.set_parameter(
            Parameter::ExtraSource(extra),
            SelectableInput::Band(BandComponent::Bulge).index() as f32,
        );
        visualizer.set_parameter(Parameter::ExtraGain(extra), 1.5);
        visualizer.toggle_contribution_mix(Contribution::Extra(extra));

        let tuning = visualizer.tuning();
        let text = ron::to_string(&tuning).expect("visualizer tuning serializes");
        let decoded: VisualizerTuning = ron::from_str(&text).expect("visualizer tuning loads");
        assert_eq!(decoded, tuning);

        let mut restored = Visualizer::new();
        restored.set_tuning(decoded.clone());
        assert_eq!(restored.tuning(), decoded);
        assert_eq!(restored.parameter_value(Parameter::KickDecay), 7.5);
        assert_eq!(
            restored.parameter_value(Parameter::Gain(Gain::SpinBand)),
            3.25
        );
        assert_eq!(
            restored.parameter_value(Parameter::BandSource(BandComponent::Spin)),
            9.0
        );
        assert_eq!(restored.parameter_value(Parameter::Constant(base)), 0.4);
        assert!(restored.property_muted(Property::Twist));
        assert!(restored.property_soloed(Property::Spin));
        let restored_extra = restored
            .mixer
            .extras(MixPoint::SpinSpeed)
            .next()
            .expect("the extra input is restored");
        assert_eq!(restored_extra.1.input, Input::Band(BandComponent::Bulge));
        assert_eq!(restored_extra.1.gain, 1.5);
        assert_eq!(restored_extra.1.mix, Mix::Mul);
    }

    #[test]
    fn adaptive_reference_drift_is_not_a_persisted_setting() {
        let mut visualizer = Visualizer::new();
        let before = visualizer.tuning();
        for _ in 0..60 {
            visualizer.band_shapes[0].observe(0.9, 1.0 / 60.0);
        }
        assert_ne!(visualizer.band_shapes[0].low, 0.0);
        assert_eq!(visualizer.tuning(), before);
    }

    #[test]
    fn a_pinned_style_value_reaches_every_spawned_figure() {
        let mut visualizer = Visualizer::new();
        visualizer.set_parameter(Parameter::Style(StyleInput::DeformBias), 1.25);
        visualizer.handle_action(Action::Drop(1.0), 0.5);
        assert!(
            (visualizer.scene.current.target_style.deform_bias - 1.25).abs() < 1e-6,
            "the drop keeps the tuned style value"
        );
    }

    #[test]
    fn component_gains_change_the_property_value() {
        let mut visualizer = Visualizer::new();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        visualizer.debug_music([0.6; BAND_COUNT], 0.6);
        visualizer.update(
            frame,
            ShutterFrames::try_from(2.0).unwrap(),
            1.0 / 60.0,
            &palette,
            (1280.0, 440.0),
            1.0,
            1.0,
        );
        let before = visualizer.debug_snapshot().properties[Property::Spin.index()];
        visualizer.set_parameter(Parameter::Gain(Gain::SpinPower), 0.0);
        visualizer.set_parameter(Parameter::Gain(Gain::SpinBand), 0.0);
        frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        visualizer.update(
            frame,
            ShutterFrames::try_from(2.0).unwrap(),
            1.0 / 60.0,
            &palette,
            (1280.0, 440.0),
            1.0,
            1.0,
        );
        let after = visualizer.debug_snapshot().properties[Property::Spin.index()];
        assert!(after < before);
        let jitter = Parameter::Gain(Gain::JitterBand);
        visualizer.set_parameter(jitter, 100.0);
        assert_eq!(visualizer.parameter_value(jitter), 10.0);
        let spin_gain = Parameter::Gain(Gain::SpinBand);
        assert_eq!(spin_gain.range().1, 10.0);
        visualizer.set_parameter(spin_gain, 100.0);
        assert_eq!(visualizer.parameter_value(spin_gain), 10.0);
    }

    #[test]
    fn mix_mode_and_extra_inputs_change_the_live_property() {
        let mut visualizer = Visualizer::new();
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        visualizer.debug_music([0.6; BAND_COUNT], 0.6);
        let update = |visualizer: &mut Visualizer, frame: AudioInterval| {
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
        };
        update(&mut visualizer, frame);
        let plain = visualizer.debug_snapshot().properties[Property::Spin.index()];

        let power = Contribution::Fixed(MixPoint::SpinSpeed, 0);
        visualizer.toggle_contribution_mix(power);
        frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        update(&mut visualizer, frame);
        let mixed = visualizer.debug_snapshot().properties[Property::Spin.index()];
        assert_ne!(plain, mixed);

        let extra = visualizer.add_extra(MixPoint::SpinSpeed);
        visualizer.set_parameter(Parameter::ExtraGain(extra), 2.0);
        frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        update(&mut visualizer, frame);
        let modulated = visualizer.debug_snapshot().properties[Property::Spin.index()];
        assert_ne!(mixed, modulated);
        visualizer.remove_extra(extra);
        assert_eq!(visualizer.mixer().extras(MixPoint::SpinSpeed).count(), 0);
    }

    #[test]
    fn band_components_can_be_rerouted() {
        let mut visualizer = Visualizer::new();
        let detectors = visualizer.detector_settings();
        let original = visualizer.band_for(BandComponent::Spin);
        assert!(Property::Spin.depends_on_band(original, visualizer.routing(), &detectors));
        visualizer.set_parameter(Parameter::BandSource(BandComponent::Spin), 7.0);
        assert_eq!(visualizer.band_for(BandComponent::Spin), 7);
        assert!(!Property::Spin.depends_on_band(original, visualizer.routing(), &detectors));
        assert!(Property::Spin.depends_on_band(7, visualizer.routing(), &detectors));
        assert_eq!(
            Property::Spin.component_gains(Input::Band(BandComponent::Spin)),
            vec![Gain::SpinBand]
        );
        let shape = visualizer
            .input_shape(Input::Band(BandComponent::Spin))
            .expect("band input has a shape");
        assert_eq!(shape.low, 0.0);
        assert_eq!(shape.high, 1.0);
        visualizer.set_parameter(
            Parameter::InputLow(EnvelopeTarget::Band(BandComponent::Spin)),
            0.4,
        );
        assert_eq!(
            visualizer
                .input_shape(Input::Band(BandComponent::Spin))
                .expect("band input has a shape")
                .low,
            0.4
        );
    }

    #[test]
    fn every_property_has_a_source_a_description_and_a_reachable_band() {
        for &property in Property::VARIANTS {
            assert!(!property.dependencies().is_empty(), "{}", property.name());
            assert!(!property.description().is_empty(), "{}", property.name());
            let (minimum, maximum) = property.range();
            assert!(
                minimum < maximum,
                "{} has an empty plot range",
                property.name()
            );
        }
        let routing = BandRouting::default();
        let detectors = DetectorSettings::default();
        for band in 0..BAND_COUNT {
            assert!(
                Property::VARIANTS
                    .iter()
                    .copied()
                    .any(|property| property.depends_on_band(band, &routing, &detectors)),
                "band {band} must reach at least one animation property"
            );
        }
        assert!(Property::Kick.depends_on_band_onset(0, &detectors));
        assert!(!Property::Flip.depends_on_band_onset(0, &detectors));
        assert!(Property::Flip.depends_on_band_onset(12, &detectors));
    }

    #[test]
    fn style_axis_crosses_opposite_targets_without_snapping() {
        let mut style = Style::default();
        let target = Style {
            axis: style.axis * -1.0,
            ..style
        };
        let mut previous = style.axis;
        for _ in 0..120 {
            style.approach(target, 1.0 / 60.0, 4.0);
            assert!((style.axis - previous).length() < 0.2);
            previous = style.axis;
        }
    }

    #[test]
    fn envelope_smoothing_is_monotonic_and_bounded() {
        let mut envelope = Envelope::new(80.0, 5.0);
        let mut previous = 0.0;
        for _ in 0..60 {
            envelope.advance(1.0, 1.0 / 120.0);
            assert!(envelope.value >= previous);
            assert!(envelope.value <= 1.0);
            previous = envelope.value;
        }
        for _ in 0..120 {
            envelope.advance(0.0, 1.0 / 120.0);
            assert!(envelope.value <= previous);
            assert!(envelope.value >= 0.0);
            previous = envelope.value;
        }
    }

    #[test]
    fn presentation_cycles_gauges_figures_and_weapon() {
        assert_eq!(Presentation::Gauges.next(), Presentation::Visualizer);
        assert_eq!(Presentation::Visualizer.next(), Presentation::Weapon);
        assert_eq!(Presentation::Weapon.next(), Presentation::Gauges);
        assert!(Presentation::Visualizer.shows_visualizer());
        assert!(Presentation::Weapon.shows_visualizer());
        assert!(!Presentation::Gauges.shows_visualizer());
    }

    #[test]
    fn crease_threshold_steps_and_clamps() {
        let mut threshold = CreaseThreshold::default();
        assert_eq!(threshold.whole_degrees(), 45);
        assert_eq!(threshold.adjusted(Adjustment::Decrease).whole_degrees(), 40);
        for _ in 0..20 {
            threshold = threshold.adjusted(Adjustment::Increase);
        }
        assert_eq!(threshold, CreaseThreshold::MAX);
        for _ in 0..20 {
            threshold = threshold.adjusted(Adjustment::Decrease);
        }
        assert_eq!(threshold, CreaseThreshold::MIN);
        assert!(CreaseThreshold::try_from(9.0).is_err());
        assert!(CreaseThreshold::try_from(81.0).is_err());
        assert!(CreaseThreshold::try_from(f32::NAN).is_err());
        assert_eq!(f32::from(CreaseThreshold::default()), 45.0);
    }

    #[test]
    fn drops_and_the_hotkey_advance_the_showcase_model() {
        let mut visualizer = Visualizer::new();
        visualizer.set_presentation(Presentation::Weapon);
        let (_, total) = visualizer
            .model_position()
            .expect("the showcase has loaded models");
        if total < 2 {
            return;
        }
        let first = visualizer.scene_name().to_owned();
        visualizer.handle_action(Action::Drop(1.0), 0.0);
        assert_ne!(visualizer.scene_name(), first);
        assert!(visualizer.scene.outgoing.is_some());
        visualizer.next_model();
        assert_eq!(visualizer.scene_name(), first);
    }

    #[test]
    fn line_width_steps_and_clamps() {
        let mut width = LineWidth::default();
        assert_eq!(width.percent(), 100);
        assert_eq!(width.adjusted(Adjustment::Decrease).percent(), 90);
        for _ in 0..40 {
            width = width.adjusted(Adjustment::Increase);
        }
        assert_eq!(width.factor(), LineWidth::MAX);
        for _ in 0..40 {
            width = width.adjusted(Adjustment::Decrease);
        }
        assert_eq!(width.factor(), LineWidth::MIN);
        assert!(LineWidth::try_from(0.09).is_err());
        assert!(LineWidth::try_from(3.1).is_err());
        assert!(LineWidth::try_from(f32::NAN).is_err());
    }

    #[test]
    fn motion_opacity_steps_and_clamps() {
        let mut opacity = MotionOpacity::default();
        assert_eq!(opacity.percent(), 35);
        assert_eq!(opacity.adjusted(Adjustment::Decrease).percent(), 30);
        for _ in 0..20 {
            opacity = opacity.adjusted(Adjustment::Decrease);
        }
        assert_eq!(opacity.value(), MotionOpacity::MIN);
        for _ in 0..30 {
            opacity = opacity.adjusted(Adjustment::Increase);
        }
        assert_eq!(opacity.value(), MotionOpacity::MAX);
        assert!(MotionOpacity::try_from(-0.01).is_err());
        assert!(MotionOpacity::try_from(1.01).is_err());
        assert!(MotionOpacity::try_from(f32::NAN).is_err());
    }

    #[test]
    fn motion_fade_power_steps_and_clamps() {
        let mut power = MotionFadePower::default();
        assert!((power.adjusted(Adjustment::Decrease).power() - 0.9).abs() < 1e-6);
        assert!((power.adjusted(Adjustment::Increase).power() - 1.1).abs() < 1e-6);
        for _ in 0..50 {
            power = power.adjusted(Adjustment::Decrease);
        }
        assert_eq!(power.power(), MotionFadePower::MIN);
        for _ in 0..50 {
            power = power.adjusted(Adjustment::Increase);
        }
        assert_eq!(power.power(), MotionFadePower::MAX);
        assert!(MotionFadePower::try_from(0.09).is_err());
        assert!(MotionFadePower::try_from(4.1).is_err());
        assert!(MotionFadePower::try_from(f32::NAN).is_err());
    }

    #[test]
    fn higher_fade_power_lowers_the_mid_speed_opacity() -> anyhow::Result<()> {
        let opacity = MotionOpacity::try_from(0.35)?;
        let base = MotionFade {
            opacity,
            power: MotionFadePower::try_from(1.0)?,
        };
        let late = MotionFade {
            opacity,
            power: MotionFadePower::try_from(1.5)?,
        };
        assert!((base.factor(0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((late.factor(0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!(late.factor(1.0, 1.0) < base.factor(1.0, 1.0));
        assert!((base.factor(1_000_000.0, 1.0) - 0.35).abs() < 1e-4);
        assert!((late.factor(1_000_000.0, 1.0) - 0.35).abs() < 1e-4);

        let clear = MotionFade {
            opacity: MotionOpacity::try_from(0.0)?,
            power: MotionFadePower::try_from(4.0)?,
        };
        assert!(clear.factor(1_000_000.0, 1.0).abs() < 1e-4);
        Ok(())
    }

    #[test]
    fn model_motion_blur_can_be_toggled() -> anyhow::Result<()> {
        let mut visualizer = Visualizer::new();
        visualizer.set_presentation(Presentation::Weapon);
        let palette = Palette::from_theme(ThemeId::default());
        let shutter = ShutterFrames::try_from(4.0)?;
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut step = |visualizer: &mut Visualizer| {
            visualizer.debug_music([0.6; BAND_COUNT], 0.6);
            visualizer.update(
                frame,
                shutter,
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        };

        step(&mut visualizer);
        assert!(!visualizer.frame().model_lines.is_empty());
        assert!(visualizer.frame().model_ribbons.is_empty());

        visualizer.set_model_motion(ModelMotion::Swept);
        for _ in 0..4 {
            step(&mut visualizer);
        }
        assert!(
            !visualizer.frame().model_ribbons.is_empty(),
            "fast model edges should sweep"
        );
        assert!(
            visualizer.line_count() > 1_000,
            "the logical line count survives the ribbon render path"
        );

        visualizer.set_model_motion(ModelMotion::Still);
        step(&mut visualizer);
        assert!(!visualizer.frame().model_lines.is_empty());
        assert!(visualizer.frame().model_ribbons.is_empty());
        Ok(())
    }

    #[test]
    fn slow_showcase_edges_use_the_analytic_line_fallback() -> anyhow::Result<()> {
        let mut visualizer = Visualizer::new();
        visualizer.set_presentation(Presentation::Weapon);
        visualizer.set_model_motion(ModelMotion::Swept);
        let palette = Palette::from_theme(ThemeId::default());
        let shutter = ShutterFrames::try_from(1.0)?;
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        for _ in 0..30 {
            visualizer.debug_music([0.0; BAND_COUNT], 0.0);
            visualizer.update(
                frame,
                shutter,
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        let frame = visualizer.frame();
        assert!(
            !frame.model_lines.is_empty(),
            "subpixel motion should fall back to analytic lines"
        );
        assert!(
            frame.model_ribbons.is_empty(),
            "subpixel motion should not emit swept ribbons"
        );
        Ok(())
    }

    #[test]
    fn line_width_scales_the_model_strokes() -> anyhow::Result<()> {
        let mut visualizer = Visualizer::new();
        visualizer.set_presentation(Presentation::Weapon);
        let palette = Palette::from_theme(ThemeId::default());
        let frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let step = |visualizer: &mut Visualizer| {
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).expect("valid shutter"),
                1.0 / 60.0,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            visualizer.frame().model_lines[0].style[0]
        };

        visualizer.set_line_width(LineWidth::try_from(1.0)?);
        let normal = step(&mut visualizer);
        visualizer.set_line_width(LineWidth::try_from(2.0)?);
        let wide = step(&mut visualizer);
        assert!((wide / normal - 2.0).abs() < 1e-4);
        Ok(())
    }

    #[test]
    fn weapon_showcase_swaps_figures_for_the_model_wireframe() -> anyhow::Result<()> {
        let mut visualizer = Visualizer::new();
        visualizer.set_presentation(Presentation::Weapon);
        assert_eq!(visualizer.presentation, Presentation::Weapon);
        assert_ne!(visualizer.scene_name(), "Cube");
        assert!(!visualizer.scene_name().is_empty());
        assert!(visualizer.scene.current.is_showcase());

        let palette = Palette::from_theme(ThemeId::default());
        let frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        visualizer.debug_music([0.6; BAND_COUNT], 0.6);
        visualizer.update(
            frame,
            ShutterFrames::try_from(2.0)?,
            1.0 / 60.0,
            &palette,
            (1280.0, 440.0),
            1.0,
            1.0,
        );
        let fine = {
            let displayed = visualizer.frame();
            let lines = displayed.lines.len() + displayed.model_lines.len();
            assert!(lines > 1_000);
            assert!(
                displayed.ribbons.is_empty(),
                "the rigid model does not sweep"
            );
            assert!(
                displayed
                    .model_lines
                    .iter()
                    .all(|line| line.style[1] == 1.0),
                "the model wireframe uses flat caps"
            );
            assert!(
                displayed.lines.iter().all(|line| line.style[1] == 0.0),
                "diagnosis lines keep round caps"
            );
            lines
        };
        visualizer.set_crease_threshold(CreaseThreshold::try_from(70.0)?);
        let next = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        visualizer.update(
            next,
            ShutterFrames::try_from(2.0)?,
            1.0 / 60.0,
            &palette,
            (1280.0, 440.0),
            1.0,
            1.0,
        );
        let coarse = visualizer.frame();
        assert!(coarse.lines.len() + coarse.model_lines.len() < fine);

        visualizer.set_presentation(Presentation::Visualizer);
        assert!(!visualizer.scene.current.is_showcase());
        assert_eq!(visualizer.scene_name(), "Cube");
        Ok(())
    }
}
