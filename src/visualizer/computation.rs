use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use strum::{EnumCount, VariantArray};

use super::{style_value, Property, Style, BAND_COUNT};

/// Base term of the line-energy mix. The visualizer scales a near-opacity
/// floor below this value against the line energy.
pub const LINE_ENERGY_BASE: f32 = 0.6;

/// Reject values that the inspector could never produce.
pub(super) fn validate_value(
    value: f32,
    (minimum, maximum): (f32, f32),
    name: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        value.is_finite() && (minimum..=maximum).contains(&value),
        "{name} must be finite and between {minimum} and {maximum}"
    );
    Ok(())
}

/// One non-analysis value that participates in a property computation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StyleInput {
    AxisX,
    AxisY,
    AxisZ,
    PhaseRate,
    TwistSign,
    ShearSign,
    DeformBias,
    ZoomBias,
    JitterBias,
}

/// Onset detector that can be selected in a computation tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Detector {
    Flux,
    Kick,
    Snare,
    Hat,
    Drop,
}

impl Detector {
    pub fn name(self) -> &'static str {
        match self {
            Self::Flux => "FLUX ONSET",
            Self::Kick => "KICK",
            Self::Snare => "SNARE",
            Self::Hat => "HAT",
            Self::Drop => "DROP",
        }
    }

    /// Parameters for a detector selected in the tree. The per-band onset
    /// analyzer builds its own parameter list.
    pub fn parameters(self) -> Vec<Parameter> {
        match self {
            Self::Flux => unreachable!("the flux analyzer has its own panel"),
            Self::Kick => vec![
                Parameter::PatternArea(Detector::Kick),
                Parameter::OnsetFlux(Detector::Kick, FluxSetting::PreGain),
                Parameter::OnsetFlux(Detector::Kick, FluxSetting::Attack),
                Parameter::OnsetFlux(Detector::Kick, FluxSetting::Release),
                Parameter::OnsetFlux(Detector::Kick, FluxSetting::Average),
                Parameter::OnsetFlux(Detector::Kick, FluxSetting::Sensitivity),
                Parameter::OnsetFlux(Detector::Kick, FluxSetting::MinThreshold),
                Parameter::OnsetFlux(Detector::Kick, FluxSetting::MinInterval),
                Parameter::OnsetSuppression(Detector::Kick),
                Parameter::KickSnareChoke,
                Parameter::KickDropChoke,
                Parameter::KickQuakeFactor,
                Parameter::KickSequenceEvery,
                Parameter::QuakeStiffness,
                Parameter::QuakeDamping,
            ],
            Self::Snare => vec![
                Parameter::PatternArea(Detector::Snare),
                Parameter::OnsetFlux(Detector::Snare, FluxSetting::PreGain),
                Parameter::OnsetFlux(Detector::Snare, FluxSetting::Attack),
                Parameter::OnsetFlux(Detector::Snare, FluxSetting::Release),
                Parameter::OnsetFlux(Detector::Snare, FluxSetting::Average),
                Parameter::OnsetFlux(Detector::Snare, FluxSetting::Sensitivity),
                Parameter::OnsetFlux(Detector::Snare, FluxSetting::MinThreshold),
                Parameter::OnsetFlux(Detector::Snare, FluxSetting::MinInterval),
                Parameter::OnsetSuppression(Detector::Snare),
                Parameter::SnareFlipEvery,
                Parameter::SnarePoiSwitches,
                Parameter::PoiSwitchGap,
                Parameter::SnareChoke,
                Parameter::FlipStiffness,
                Parameter::FlipDamping,
            ],
            Self::Hat => vec![
                Parameter::PatternArea(Detector::Hat),
                Parameter::OnsetFlux(Detector::Hat, FluxSetting::PreGain),
                Parameter::OnsetFlux(Detector::Hat, FluxSetting::Attack),
                Parameter::OnsetFlux(Detector::Hat, FluxSetting::Release),
                Parameter::OnsetFlux(Detector::Hat, FluxSetting::Average),
                Parameter::OnsetFlux(Detector::Hat, FluxSetting::Sensitivity),
                Parameter::OnsetFlux(Detector::Hat, FluxSetting::MinThreshold),
                Parameter::OnsetFlux(Detector::Hat, FluxSetting::MinInterval),
                Parameter::OnsetSuppression(Detector::Hat),
            ],
            Self::Drop => vec![
                Parameter::DropGapWindow,
                Parameter::DropGapGain,
                Parameter::DropGapThreshold,
                Parameter::DropLowLp,
                Parameter::DropHighLp,
                Parameter::OnsetFlux(Detector::Drop, FluxSetting::PreGain),
                Parameter::OnsetFlux(Detector::Drop, FluxSetting::Attack),
                Parameter::OnsetFlux(Detector::Drop, FluxSetting::Release),
                Parameter::OnsetFlux(Detector::Drop, FluxSetting::Average),
                Parameter::OnsetFlux(Detector::Drop, FluxSetting::Sensitivity),
                Parameter::OnsetFlux(Detector::Drop, FluxSetting::MinThreshold),
                Parameter::OnsetFlux(Detector::Drop, FluxSetting::MinInterval),
                Parameter::OnsetSuppression(Detector::Drop),
                Parameter::DropQuakeFactor,
            ],
        }
    }
}

/// One setting of a detector's dedicated spectral-flux tracker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FluxSetting {
    PreGain,
    Attack,
    Release,
    Average,
    Sensitivity,
    MinThreshold,
    MinInterval,
}

impl FluxSetting {
    pub fn label(self) -> &'static str {
        match self {
            Self::PreGain => "ONS PRE",
            Self::Attack => "ONS ATTACK",
            Self::Release => "ONS RELEASE",
            Self::Average => "ONS AVERAGE",
            Self::Sensitivity => "ONS SENS",
            Self::MinThreshold => "ONS THRESH",
            Self::MinInterval => "ONS GAP",
        }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            Self::PreGain => (super::FLUX_PRE_GAIN_MIN, super::FLUX_PRE_GAIN_MAX),
            Self::Attack => (super::FLUX_ATTACK_MIN, super::FLUX_ATTACK_MAX),
            Self::Release => (super::FLUX_RELEASE_MIN, super::FLUX_RELEASE_MAX),
            Self::Average => (super::FLUX_AVERAGE_MIN, super::FLUX_AVERAGE_MAX),
            Self::Sensitivity => (super::FLUX_SENSITIVITY_MIN, super::FLUX_SENSITIVITY_MAX),
            Self::MinThreshold => (super::FLUX_THRESHOLD_MIN, super::ONSET_THRESHOLD_MAX),
            Self::MinInterval => (super::FLUX_INTERVAL_MIN, super::FLUX_INTERVAL_MAX),
        }
    }

    pub fn step(self) -> f32 {
        match self {
            Self::PreGain => 0.01,
            Self::Attack | Self::MinThreshold => 0.001,
            Self::Release | Self::Average | Self::MinInterval => 0.005,
            Self::Sensitivity => 0.05,
        }
    }

    pub fn format(self, value: f32) -> String {
        match self {
            Self::PreGain => format!("{value:.2}"),
            Self::Attack | Self::Release | Self::Average | Self::MinInterval => {
                format!("{:.0}ms", value * 1000.0)
            }
            Self::Sensitivity => format!("{value:.2}"),
            Self::MinThreshold => format!("{value:.3}"),
        }
    }

    pub fn is_time(self) -> bool {
        matches!(
            self,
            Self::Attack | Self::Release | Self::Average | Self::MinInterval
        )
    }
}

/// A semantic band input. The actual FFT band is routed by `BandRouting`.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    strum::EnumCount,
    strum::VariantArray,
)]
pub enum BandComponent {
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

impl BandComponent {
    pub fn default_band(self) -> usize {
        match self {
            Self::Spin => 2,
            Self::Pitch => 5,
            Self::Roll => 0,
            Self::Twist => 7,
            Self::Bulge => 8,
            Self::Ripple => 9,
            Self::Shear => 10,
            Self::Zoom => 11,
            Self::Accent => 12,
            Self::Line => 13,
            Self::Jitter => 14,
            Self::Phase => 15,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
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

    pub fn index(self) -> usize {
        Self::VARIANTS
            .iter()
            .position(|component| *component == self)
            .expect("every band component is listed")
    }
}

/// Maps each semantic band input onto one of the sixteen FFT bands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandRouting([usize; BandComponent::COUNT]);

impl Default for BandRouting {
    fn default() -> Self {
        Self(std::array::from_fn(|index| {
            BandComponent::VARIANTS[index].default_band()
        }))
    }
}

impl std::ops::Index<BandComponent> for BandRouting {
    type Output = usize;

    fn index(&self, component: BandComponent) -> &usize {
        &self.0[component.index()]
    }
}

impl std::ops::IndexMut<BandComponent> for BandRouting {
    fn index_mut(&mut self, component: BandComponent) -> &mut usize {
        &mut self.0[component.index()]
    }
}

impl BandRouting {
    pub fn validate(self) -> anyhow::Result<()> {
        for &component in BandComponent::VARIANTS {
            anyhow::ensure!(
                self[component] < BAND_COUNT,
                "{} must be routed to a band below {BAND_COUNT}",
                component.label()
            );
        }
        Ok(())
    }
}

impl Serialize for BandRouting {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;
        for &component in BandComponent::VARIANTS {
            let band = self[component];
            if band != component.default_band() {
                map.serialize_entry(&component, &band)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for BandRouting {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let overrides = BTreeMap::<BandComponent, usize>::deserialize(deserializer)?;
        let mut routing = Self::default();
        for (component, band) in overrides {
            if band >= BAND_COUNT {
                return Err(serde::de::Error::custom(format!(
                    "{} must be routed to a band below {BAND_COUNT}",
                    component.label()
                )));
            }
            routing[component] = band;
        }
        Ok(routing)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnvelopeTarget {
    Band(BandComponent),
    /// A raw FFT band selected in the onset flow analyzer.
    BandIndex(usize),
    Power,
    Bass,
    Treble,
}

/// One multiplier attached to an incoming component.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    strum::EnumCount,
    strum::VariantArray,
)]
pub enum Gain {
    PulsePower,
    PulseBass,
    PulseKick,
    PulseEntrance,
    ZoomBand,
    SpinPower,
    SpinBand,
    PitchBand,
    PitchOscillator,
    PitchPowerRate,
    RollBand,
    RollPowerRate,
    TwistBand,
    BulgeBand,
    RippleBand,
    ShearBand,
    LineEnergy,
    JitterBand,
    PhasePower,
    PhaseBand,
    XfadePower,
    XfadeKick,
    MorphPower,
    EntrancePower,
}

impl Gain {
    pub fn default_value(self) -> f32 {
        match self {
            Self::PulsePower => 0.18,
            Self::PulseBass => 0.11,
            Self::PulseKick => 0.12,
            Self::PulseEntrance => 0.38,
            Self::ZoomBand => 0.18,
            Self::SpinPower => 1.9,
            Self::SpinBand => 6.02,
            Self::PitchBand => 0.55,
            Self::PitchOscillator => 0.08,
            Self::PitchPowerRate => 1.8,
            Self::RollBand => 1.75,
            Self::RollPowerRate => 1.6,
            Self::TwistBand => 1.01,
            Self::BulgeBand => 1.01,
            Self::RippleBand => 0.75,
            Self::ShearBand => 0.55,
            Self::LineEnergy => 0.4,
            Self::JitterBand => 0.055,
            Self::PhasePower => 2.6,
            Self::PhaseBand => 0.7,
            Self::XfadePower => 2.8,
            Self::XfadeKick => 1.7,
            Self::MorphPower => 3.0,
            Self::EntrancePower => 2.5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PulsePower | Self::SpinPower | Self::PhasePower | Self::XfadePower => "PWR GAIN",
            Self::PulseBass => "BASS GAIN",
            Self::PulseKick | Self::XfadeKick => "KICK GAIN",
            Self::PulseEntrance => "ENTRANCE GAIN",
            Self::ZoomBand => "ZOOM BAND GAIN",
            Self::SpinBand => "SPIN BAND GAIN",
            Self::PitchBand => "PITCH BAND GAIN",
            Self::RollBand => "ROLL BAND GAIN",
            Self::PitchOscillator => "OSC GAIN",
            Self::PitchPowerRate | Self::RollPowerRate => "PWR RATE GAIN",
            Self::TwistBand => "TWIST BAND GAIN",
            Self::BulgeBand => "BULGE BAND GAIN",
            Self::RippleBand => "RIPPLE BAND GAIN",
            Self::ShearBand => "SHEAR BAND GAIN",
            Self::LineEnergy => "LINE BAND GAIN",
            Self::JitterBand => "JITTER BAND GAIN",
            Self::PhaseBand => "PHASE BAND GAIN",
            Self::MorphPower | Self::EntrancePower => "PWR GAIN",
        }
    }

    pub fn band_component(self) -> Option<BandComponent> {
        match self {
            Self::ZoomBand => Some(BandComponent::Zoom),
            Self::SpinBand => Some(BandComponent::Spin),
            Self::PitchBand => Some(BandComponent::Pitch),
            Self::RollBand => Some(BandComponent::Roll),
            Self::TwistBand => Some(BandComponent::Twist),
            Self::BulgeBand => Some(BandComponent::Bulge),
            Self::RippleBand => Some(BandComponent::Ripple),
            Self::ShearBand => Some(BandComponent::Shear),
            Self::LineEnergy => Some(BandComponent::Line),
            Self::JitterBand => Some(BandComponent::Jitter),
            Self::PhaseBand => Some(BandComponent::Phase),
            _ => None,
        }
    }

    pub fn range(self) -> (f32, f32) {
        if self.band_component().is_some() {
            (0.0, 10.0)
        } else {
            (0.0, 5.0)
        }
    }

    pub fn step(self) -> f32 {
        match self {
            Self::JitterBand => 0.001,
            _ => 0.01,
        }
    }

    pub fn index(self) -> usize {
        Self::VARIANTS
            .iter()
            .position(|gain| *gain == self)
            .expect("every gain variant is listed")
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gains([f32; Gain::COUNT]);

impl Default for Gains {
    fn default() -> Self {
        Self(std::array::from_fn(|index| {
            Gain::VARIANTS[index].default_value()
        }))
    }
}

impl std::ops::Index<Gain> for Gains {
    type Output = f32;

    fn index(&self, gain: Gain) -> &f32 {
        &self.0[gain.index()]
    }
}

impl std::ops::IndexMut<Gain> for Gains {
    fn index_mut(&mut self, gain: Gain) -> &mut f32 {
        &mut self.0[gain.index()]
    }
}

impl Gains {
    pub fn validate(self) -> anyhow::Result<()> {
        for &gain in Gain::VARIANTS {
            validate_value(self[gain], gain.range(), &format!("{gain:?} gain"))?;
        }
        Ok(())
    }
}

impl Serialize for Gains {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;
        for &gain in Gain::VARIANTS {
            let value = self[gain];
            if value != gain.default_value() {
                map.serialize_entry(&gain, &value)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Gains {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let overrides = BTreeMap::<Gain, f32>::deserialize(deserializer)?;
        let mut gains = Self::default();
        for (gain, value) in overrides {
            let (minimum, maximum) = gain.range();
            if !value.is_finite() || !(minimum..=maximum).contains(&value) {
                return Err(serde::de::Error::custom(format!(
                    "{gain:?} gain must be finite and between {minimum} and {maximum}"
                )));
            }
            gains[gain] = value;
        }
        Ok(gains)
    }
}

/// Pre-envelope response curve applied to a normalized 0..1 input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Curve {
    Linear,
    Inflated,
    Deflated,
    SCurve,
}

impl Curve {
    /// Upper bound of the curve amount. The S-curve applies one smoothstep
    /// per unit, so the slider range also bounds the iteration count.
    pub const MAX_AMOUNT: f32 = 8.0;

    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Inflated,
            2 => Self::Deflated,
            3 => Self::SCurve,
            _ => Self::Linear,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Linear => 0,
            Self::Inflated => 1,
            Self::Deflated => 2,
            Self::SCurve => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "LINEAR",
            Self::Inflated => "INFLATED",
            Self::Deflated => "DEFLATED",
            Self::SCurve => "S-CURVE",
        }
    }

    pub fn apply(self, value: f32, amount: f32) -> f32 {
        assert!(
            (0.0..=Self::MAX_AMOUNT).contains(&amount),
            "curve amount {amount} is outside 0..={}",
            Self::MAX_AMOUNT
        );
        match self {
            Self::Linear => value,
            // The exponent keeps the square-root bend at amount 1 and keeps
            // lifting lower values as the amount grows.
            Self::Inflated => value.powf(1.0 / (1.0 + amount)),
            // The exponent keeps the square bend at amount 1 and keeps
            // pressing lower values down as the amount grows.
            Self::Deflated => value.powf(1.0 + amount),
            // Every whole unit applies the smoothstep again; the remainder
            // blends toward the next pass so the amount stays continuous.
            Self::SCurve => {
                let whole = amount.floor();
                let mut shaped = value;
                for _ in 0..whole as usize {
                    shaped = super::smoothstep(shaped);
                }
                let fraction = amount - whole;
                if fraction > 0.0 {
                    shaped += (super::smoothstep(shaped) - shaped) * fraction;
                }
                shaped
            }
        }
    }
}

/// How an input shape chooses its low/high references.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceTracking {
    #[default]
    Manual,
    Adaptive,
}

impl ReferenceTracking {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Manual => Self::Adaptive,
            Self::Adaptive => Self::Manual,
        }
    }
}

/// Rescales a raw analysis value into 0..1 and optionally bends it before the
/// envelope smoothing stage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputShape {
    pub low: f32,
    pub high: f32,
    pub curve: Curve,
    /// How far the curve departs from linear. Zero is linear, one is the
    /// named bend, and larger values keep bending.
    pub amount: f32,
    /// When adaptive, the low/high references follow the incoming values
    /// instead of staying at the slider positions.
    pub tracking: ReferenceTracking,
}

impl Default for InputShape {
    fn default() -> Self {
        Self {
            low: 0.0,
            high: 1.0,
            curve: Curve::Linear,
            amount: 1.0,
            tracking: ReferenceTracking::Manual,
        }
    }
}

impl InputShape {
    /// An input whose references follow the incoming level. Every FFT band
    /// starts this way so quiet and loud material share the same onset scale.
    pub fn adaptive() -> Self {
        Self {
            tracking: ReferenceTracking::Adaptive,
            ..Self::default()
        }
    }
}

impl InputShape {
    /// The high reference is bumped up quickly by peaks but falls slowly.
    const HIGH_RISE: f32 = 8.0;
    const HIGH_FALL: f32 = 0.35;
    /// The low reference lifts slowly but is pushed down quickly by low values.
    const LOW_RISE: f32 = 0.18;
    const LOW_FALL: f32 = 3.0;
    const MIN_SPAN: f32 = 0.01;

    pub fn is_adaptive(self) -> bool {
        matches!(self.tracking, ReferenceTracking::Adaptive)
    }

    /// Updates the adaptive references for one hop without shaping the value.
    pub fn track(&mut self, value: f32, dt: f32) {
        if self.is_adaptive() {
            self.adapt(value, dt);
        }
    }

    /// Rescales the value between the low/high references without applying the
    /// curve.
    pub fn normalized(self, value: f32) -> f32 {
        assert!(
            self.low < self.high,
            "input shape reference points must be ordered"
        );
        ((value - self.low) / (self.high - self.low)).clamp(0.0, 1.0)
    }

    pub fn apply(self, value: f32) -> f32 {
        self.curve.apply(self.normalized(value), self.amount)
    }

    /// Advances one raw input sample. An adaptive shape tracks the references
    /// before scaling, so the following envelope sees a slowly normalized signal
    /// rather than a value that jumps when the user drags a reference.
    pub fn observe(&mut self, value: f32, dt: f32) -> f32 {
        self.track(value, dt);
        self.apply(value)
    }

    fn adapt(&mut self, value: f32, dt: f32) {
        let value = value.clamp(0.0, 1.0);
        let high_rate = if value > self.high {
            Self::HIGH_RISE
        } else {
            Self::HIGH_FALL
        };
        let low_rate = if value < self.low {
            Self::LOW_FALL
        } else {
            Self::LOW_RISE
        };
        self.high += (value - self.high) * (1.0 - (-high_rate * dt).exp());
        self.low += (value - self.low) * (1.0 - (-low_rate * dt).exp());
        self.low = self.low.clamp(0.0, 1.0 - Self::MIN_SPAN);
        self.high = self.high.clamp(Self::MIN_SPAN, 1.0);
        self.low = self.low.min(self.high - Self::MIN_SPAN);
        self.high = self.high.max(self.low + Self::MIN_SPAN);
    }

    pub fn toggle_adaptive(&mut self) {
        self.tracking = self.tracking.toggled();
    }

    /// The shape as it belongs in the config. Adaptive references track the
    /// incoming audio, so their live positions are not settings.
    pub fn persisted(self) -> Self {
        if self.is_adaptive() {
            Self {
                low: 0.0,
                high: 1.0,
                ..self
            }
        } else {
            self
        }
    }

    pub fn validate(self) -> anyhow::Result<()> {
        validate_value(self.low, (0.0, 0.99), "input shape low reference")?;
        validate_value(self.high, (0.01, 1.0), "input shape high reference")?;
        anyhow::ensure!(
            self.high - self.low >= Self::MIN_SPAN,
            "input shape references must be at least {} apart",
            Self::MIN_SPAN
        );
        validate_value(
            self.amount,
            (0.0, Curve::MAX_AMOUNT),
            "input shape curve amount",
        )?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Parameter {
    /// Fraction of the temporal kernel used for matching.
    PatternArea(Detector),
    /// Per-band absolute-loudness gain, curve, and amount.
    BandAblGain(usize),
    BandAblCurve(usize),
    BandAblAmount(usize),
    /// Global relative-excess background detector settings.
    ExcessAttack,
    ExcessRelease,
    ExcessFloor,
    ExcessCap,
    /// One setting of a detector's dedicated onset tracker.
    OnsetFlux(Detector, FluxSetting),
    /// How much suppression flux a detector subtracts from its onset.
    OnsetSuppression(Detector),
    KickSnareChoke,
    KickDropChoke,
    KickQuakeFactor,
    KickSequenceEvery,
    SnareFlipEvery,
    SnarePoiSwitches,
    PoiSwitchGap,
    SnareChoke,
    FlipStiffness,
    FlipDamping,
    DropGapWindow,
    DropGapGain,
    DropGapThreshold,
    DropLowLp,
    DropHighLp,
    DropQuakeFactor,
    QuakeStiffness,
    QuakeDamping,
    FocusStiffness,
    FocusDamping,
    MotionStiffness,
    MotionDamping,
    KickAttack,
    OrbitStepMin,
    OrbitStepMax,
    FovStepMin,
    FovStepMax,
    OrbitSpeed,
    DollyDepth,
    DollyPeriod,
    OrbitStiffness,
    OrbitDamping,
    DollyStiffness,
    DollyDamping,
    SelectorAttenuation,
    PoiAttenuation,
    SelectorFadeIn,
    SelectorFadeOut,
    SelectorColorChance,
    SelectorExitTriggers,
    SelectorGapTimeout,
    Attack(EnvelopeTarget),
    Release(EnvelopeTarget),
    KickDecay,
    Style(StyleInput),
    Gain(Gain),
    BandSource(BandComponent),
    InputLow(EnvelopeTarget),
    InputHigh(EnvelopeTarget),
    InputCurve(EnvelopeTarget),
    InputAmount(EnvelopeTarget),
    /// Value of a constant term in a mix point.
    Constant(Contribution),
    ExtraSource(ExtraRef),
    ExtraGain(ExtraRef),
}

impl Parameter {
    pub fn label(self) -> &'static str {
        match self {
            Self::PatternArea(_) => "KERNEL AREA",
            Self::BandAblGain(_) => "ABL GAIN",
            Self::BandAblCurve(_) => "ABL CURVE",
            Self::BandAblAmount(_) => "ABL AMT",
            Self::ExcessAttack => "BG ATTACK",
            Self::ExcessRelease => "BG RELEASE",
            Self::ExcessFloor => "EXCESS FLOOR",
            Self::ExcessCap => "EXCESS CAP",
            Self::OnsetFlux(_, setting) => setting.label(),
            Self::OnsetSuppression(_) => "ONS SUPPRESS",
            Self::KickSnareChoke => "SNARE CHOKE",
            Self::KickDropChoke => "DROP CHOKE",
            Self::KickQuakeFactor => "QUAKE x",
            Self::KickSequenceEvery => "SEQUENCE N",
            Self::SnareFlipEvery => "NORMAL FLIPS",
            Self::SnarePoiSwitches => "POI SWITCHES",
            Self::PoiSwitchGap => "POI GAP",
            Self::SnareChoke => "CHOKE s",
            Self::FlipStiffness => "FLIP K",
            Self::FlipDamping => "FLIP D",
            Self::DropGapWindow => "GAP WIN s",
            Self::DropGapGain => "GAP GAIN",
            Self::DropGapThreshold => "GAP CONF",
            Self::DropLowLp => "LF LP",
            Self::DropHighLp => "HF LP",
            Self::DropQuakeFactor => "QUAKE x",
            Self::QuakeStiffness => "QUAKE K",
            Self::QuakeDamping => "QUAKE D",
            Self::FocusStiffness => "FOCUS K",
            Self::FocusDamping => "FOCUS D",
            Self::MotionStiffness => "MOTION K",
            Self::MotionDamping => "MOTION D",
            Self::KickAttack => "KICK ATTACK",
            Self::OrbitStepMin => "ORBIT MIN",
            Self::OrbitStepMax => "ORBIT MAX",
            Self::FovStepMin => "FOV MIN",
            Self::FovStepMax => "FOV MAX",
            Self::OrbitSpeed => "ORBIT SPEED",
            Self::DollyDepth => "DOLLY DEPTH",
            Self::DollyPeriod => "DOLLY TIME",
            Self::OrbitStiffness => "ORBIT K",
            Self::OrbitDamping => "ORBIT D",
            Self::DollyStiffness => "DOLLY K",
            Self::DollyDamping => "DOLLY D",
            Self::SelectorAttenuation => "SEL ATTEN",
            Self::PoiAttenuation => "POI ATTEN",
            Self::SelectorFadeIn => "SEL FADE IN",
            Self::SelectorFadeOut => "SEL FADE OUT",
            Self::SelectorColorChance => "SEL COLOR %",
            Self::SelectorExitTriggers => "SEL EXIT N",
            Self::SelectorGapTimeout => "SEL GAP s",
            Self::Attack(_) => "ATTACK",
            Self::Release(_) => "RELEASE",
            Self::KickDecay => "DECAY",
            Self::Style(StyleInput::AxisX) => "AXIS X",
            Self::Style(StyleInput::AxisY) => "AXIS Y",
            Self::Style(StyleInput::AxisZ) => "AXIS Z",
            Self::Style(StyleInput::PhaseRate) => "PHASE RATE",
            Self::Style(StyleInput::TwistSign) => "TWIST SIGN",
            Self::Style(StyleInput::ShearSign) => "SHEAR SIGN",
            Self::Style(StyleInput::DeformBias) => "DEFORM BIAS",
            Self::Style(StyleInput::ZoomBias) => "ZOOM BIAS",
            Self::Style(StyleInput::JitterBias) => "JITTER BIAS",
            Self::Gain(gain) => gain.label(),
            Self::BandSource(_) => "BAND",
            Self::InputLow(_) => "LOW REF",
            Self::InputHigh(_) => "HIGH REF",
            Self::InputCurve(_) => "CURVE",
            Self::InputAmount(_) => "CURVE AMT",
            Self::Constant(_) => "VALUE",
            Self::ExtraSource(_) => "INPUT",
            Self::ExtraGain(_) => "EXTRA GAIN",
        }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            Self::PatternArea(_) => (0.1, 1.0),
            Self::BandAblGain(_) => (super::ABL_GAIN_MIN, super::ABL_GAIN_MAX),
            Self::BandAblCurve(_) => (0.0, 3.0),
            Self::BandAblAmount(_) => (super::ABL_AMOUNT_MIN, Curve::MAX_AMOUNT),
            Self::ExcessAttack => (super::EXCESS_ATTACK_MIN, super::EXCESS_ATTACK_MAX),
            Self::ExcessRelease => (super::EXCESS_RELEASE_MIN, super::EXCESS_RELEASE_MAX),
            Self::ExcessFloor => (super::EXCESS_FLOOR_MIN, super::EXCESS_FLOOR_MAX),
            Self::ExcessCap => (super::EXCESS_CAP_MIN, super::EXCESS_CAP_MAX),
            Self::OnsetFlux(_, setting) => setting.range(),
            Self::OnsetSuppression(_) => (super::FLUX_SUPPRESSION_MIN, super::FLUX_SUPPRESSION_MAX),
            Self::KickSnareChoke | Self::KickDropChoke => (super::CHOKE_MIN, super::CHOKE_MAX),
            Self::KickQuakeFactor => (super::QUAKE_FACTOR_MIN, super::QUAKE_FACTOR_MAX),
            Self::KickSequenceEvery => (
                super::SEQUENCE_EVERY_MIN as f32,
                super::SEQUENCE_EVERY_MAX as f32,
            ),
            Self::SnareFlipEvery | Self::SnarePoiSwitches => (
                super::SEQUENCE_EVERY_MIN as f32,
                super::SEQUENCE_EVERY_MAX as f32,
            ),
            Self::PoiSwitchGap => (
                super::showcase::POI_SWITCH_GAP_MIN,
                super::showcase::POI_SWITCH_GAP_MAX,
            ),
            Self::SnareChoke => (super::CHOKE_MIN, super::CHOKE_MAX),
            Self::FlipStiffness
            | Self::QuakeStiffness
            | Self::FocusStiffness
            | Self::MotionStiffness
            | Self::OrbitStiffness
            | Self::DollyStiffness => (super::SPRING_STIFFNESS_MIN, super::SPRING_STIFFNESS_MAX),
            Self::FlipDamping
            | Self::QuakeDamping
            | Self::FocusDamping
            | Self::MotionDamping
            | Self::OrbitDamping
            | Self::DollyDamping => (super::SPRING_DAMPING_MIN, super::SPRING_DAMPING_MAX),
            Self::KickAttack => (
                super::showcase::KICK_ATTACK_MIN,
                super::showcase::KICK_ATTACK_MAX,
            ),
            Self::OrbitStepMin | Self::OrbitStepMax => (0.0, super::showcase::POI_ORBIT_STEP_MAX),
            Self::FovStepMin | Self::FovStepMax => (0.0, super::showcase::POI_FOV_STEP_MAX),
            Self::OrbitSpeed => (0.0, super::showcase::POI_ORBIT_SPEED_MAX),
            Self::DollyDepth => (0.0, super::showcase::POI_DOLLY_DEPTH_MAX),
            Self::DollyPeriod => (
                super::showcase::POI_DOLLY_PERIOD_MIN,
                super::showcase::POI_DOLLY_PERIOD_MAX,
            ),
            Self::SelectorAttenuation | Self::PoiAttenuation | Self::SelectorColorChance => {
                (0.0, 1.0)
            }
            Self::SelectorFadeIn => (
                super::showcase::SELECTOR_FADE_IN_MIN,
                super::showcase::SELECTOR_FADE_IN_MAX,
            ),
            Self::SelectorFadeOut => (
                super::showcase::SELECTOR_FADE_OUT_MIN,
                super::showcase::SELECTOR_FADE_OUT_MAX,
            ),
            Self::SelectorExitTriggers => (
                super::showcase::SELECTOR_EXIT_TRIGGERS_MIN as f32,
                super::showcase::SELECTOR_EXIT_TRIGGERS_MAX as f32,
            ),
            Self::SelectorGapTimeout => (
                super::showcase::SELECTOR_GAP_TIMEOUT_MIN,
                super::showcase::SELECTOR_GAP_TIMEOUT_MAX,
            ),
            Self::DropGapWindow => (super::DROP_GAP_WINDOW_MIN, super::DROP_GAP_WINDOW_MAX),
            Self::DropGapGain => (super::DROP_GAP_GAIN_MIN, super::DROP_GAP_GAIN_MAX),
            Self::DropGapThreshold => {
                (super::DROP_GAP_THRESHOLD_MIN, super::DROP_GAP_THRESHOLD_MAX)
            }
            Self::DropLowLp | Self::DropHighLp => (super::DROP_LP_MIN, super::DROP_LP_MAX),
            Self::DropQuakeFactor => (super::QUAKE_FACTOR_MIN, super::QUAKE_FACTOR_MAX),
            Self::Attack(_) => (0.1, 50.0),
            Self::Release(_) => (0.1, 20.0),
            Self::KickDecay => (0.0, 20.0),
            Self::Style(StyleInput::AxisX | StyleInput::AxisY | StyleInput::AxisZ) => (-1.0, 1.0),
            Self::Style(StyleInput::PhaseRate) => (0.1, 3.0),
            Self::Style(StyleInput::TwistSign | StyleInput::ShearSign) => (-1.0, 1.0),
            Self::Style(StyleInput::DeformBias) => (0.0, 2.0),
            Self::Style(StyleInput::ZoomBias) => (0.5, 1.5),
            Self::Style(StyleInput::JitterBias) => (0.0, 2.0),
            Self::Gain(gain) => gain.range(),
            Self::BandSource(_) => (0.0, (BAND_COUNT - 1) as f32),
            Self::InputLow(_) => (0.0, 0.99),
            Self::InputHigh(_) => (0.01, 1.0),
            Self::InputCurve(_) => (0.0, 3.0),
            Self::InputAmount(_) => (0.0, Curve::MAX_AMOUNT),
            Self::Constant(_) => (-2.0, 2.0),
            Self::ExtraSource(_) => (0.0, (SelectableInput::ALL.len() - 1) as f32),
            Self::ExtraGain(_) => (EXTRA_GAIN_MIN, EXTRA_GAIN_MAX),
        }
    }

    pub fn step(self) -> f32 {
        match self {
            Self::PatternArea(_) => 0.01,
            Self::BandAblGain(_) | Self::BandAblAmount(_) | Self::ExcessCap => 0.01,
            Self::BandAblCurve(_) => 1.0,
            Self::ExcessAttack | Self::ExcessRelease => 0.005,
            Self::ExcessFloor => 0.001,
            Self::OnsetFlux(_, setting) => setting.step(),
            Self::OnsetSuppression(_) => 0.01,
            Self::KickSequenceEvery | Self::SnareFlipEvery | Self::SnarePoiSwitches => 1.0,
            Self::PoiSwitchGap => 0.05,
            Self::DropGapWindow
            | Self::DropGapGain
            | Self::DropGapThreshold
            | Self::DropLowLp
            | Self::DropHighLp
            | Self::KickSnareChoke
            | Self::KickDropChoke
            | Self::SnareChoke
            | Self::KickQuakeFactor
            | Self::DropQuakeFactor => 0.01,
            Self::FlipStiffness
            | Self::FlipDamping
            | Self::QuakeStiffness
            | Self::QuakeDamping
            | Self::FocusStiffness
            | Self::FocusDamping
            | Self::MotionStiffness
            | Self::MotionDamping
            | Self::OrbitStiffness
            | Self::OrbitDamping
            | Self::DollyStiffness
            | Self::DollyDamping => 0.5,
            Self::SelectorFadeIn | Self::SelectorFadeOut | Self::SelectorGapTimeout => 0.005,
            Self::SelectorExitTriggers => 1.0,
            Self::KickAttack => 0.5,
            Self::OrbitStepMin | Self::OrbitStepMax | Self::FovStepMin | Self::FovStepMax => {
                std::f32::consts::PI / 180.0
            }
            Self::OrbitSpeed => std::f32::consts::PI / 360.0,
            Self::DollyDepth => 0.01,
            Self::DollyPeriod => 0.5,
            Self::SelectorAttenuation | Self::PoiAttenuation | Self::SelectorColorChance => 0.01,
            Self::Attack(_) | Self::Release(_) => 0.5,
            Self::KickDecay => 0.1,
            Self::Style(_) => 0.01,
            Self::Gain(gain) => gain.step(),
            Self::BandSource(_) | Self::InputCurve(_) | Self::ExtraSource(_) => 1.0,
            Self::InputLow(_) | Self::InputHigh(_) | Self::InputAmount(_) => 0.01,
            Self::Constant(_) => 0.01,
            Self::ExtraGain(_) => 0.01,
        }
    }

    pub fn format(self, value: f32) -> String {
        match self {
            Self::PatternArea(_) => format!("{:.0}%", value * 100.0),
            Self::BandAblGain(_) | Self::BandAblAmount(_) | Self::ExcessCap => {
                format!("{value:.2}")
            }
            Self::BandAblCurve(_) => Curve::from_index(value.round() as usize).name().to_owned(),
            Self::ExcessAttack | Self::ExcessRelease | Self::DropLowLp | Self::DropHighLp => {
                format!("{:.0}ms", value * 1000.0)
            }
            Self::ExcessFloor => format!("{value:.3}"),
            Self::OnsetFlux(_, setting) => setting.format(value),
            Self::FlipStiffness
            | Self::QuakeStiffness
            | Self::FocusStiffness
            | Self::OrbitStiffness
            | Self::DollyStiffness
            | Self::FocusDamping
            | Self::MotionStiffness
            | Self::MotionDamping
            | Self::OrbitDamping
            | Self::DollyDamping => format!("{value:.1}"),
            Self::SelectorFadeIn | Self::SelectorFadeOut => format!("{:.0}ms", value * 1000.0),
            Self::SelectorAttenuation | Self::PoiAttenuation | Self::SelectorColorChance => {
                format!("{:.0}%", value * 100.0)
            }
            Self::SelectorExitTriggers => format!("{value:.0}"),
            Self::KickAttack => format!("{value:.1}/s"),
            Self::OrbitStepMin | Self::OrbitStepMax | Self::FovStepMin | Self::FovStepMax => {
                format!("{:.0}deg", value.to_degrees())
            }
            Self::OrbitSpeed => format!("{:.1}deg/s", value.to_degrees()),
            Self::DollyDepth => format!("{:.0}%", value * 100.0),
            Self::DollyPeriod => format!("{:.1}s", value),
            Self::SelectorGapTimeout => format!("{value:.1}s"),
            Self::BandSource(_) => format!("B{:02}", value.round() as usize),
            Self::KickSequenceEvery | Self::SnareFlipEvery | Self::SnarePoiSwitches => {
                format!("{value:.0}")
            }
            Self::PoiSwitchGap => format!("{value:.2}s"),
            Self::Attack(_) | Self::Release(_) => format!("{value:.1}"),
            Self::InputCurve(_) => Curve::from_index(value.round() as usize).name().to_owned(),
            Self::ExtraSource(_) => Input::from_selectable(value.round() as usize)
                .title()
                .to_owned(),
            _ => format!("{value:.2}"),
        }
    }

    /// Slider position for a value, using a logarithmic scale for envelope
    /// times so the low end gains more travel.
    pub fn fraction(self, value: f32) -> f32 {
        let (minimum, maximum) = self.range();
        let value = value.clamp(minimum, maximum);
        match self {
            Self::DropGapGain
            | Self::Attack(_)
            | Self::Release(_)
            | Self::BandAblGain(_)
            | Self::ExcessAttack
            | Self::ExcessRelease
            | Self::OnsetFlux(_, FluxSetting::PreGain) => {
                (value / minimum).ln() / (maximum / minimum).ln()
            }
            Self::OnsetFlux(_, setting) if setting.is_time() => {
                (value / minimum).ln() / (maximum / minimum).ln()
            }
            _ => (value - minimum) / (maximum - minimum),
        }
    }

    pub fn from_fraction(self, fraction: f32) -> f32 {
        let (minimum, maximum) = self.range();
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            Self::DropGapGain
            | Self::Attack(_)
            | Self::Release(_)
            | Self::BandAblGain(_)
            | Self::ExcessAttack
            | Self::ExcessRelease
            | Self::OnsetFlux(_, FluxSetting::PreGain) => {
                let fraction = (fraction * 256.0).round() / 256.0;
                minimum * (maximum / minimum).powf(fraction)
            }
            Self::OnsetFlux(_, setting) if setting.is_time() => {
                let fraction = (fraction * 256.0).round() / 256.0;
                minimum * (maximum / minimum).powf(fraction)
            }
            Self::BandSource(_) => minimum + (fraction * (maximum - minimum)).round(),
            _ => {
                let value = minimum + fraction * (maximum - minimum);
                let step = self.step();
                (minimum + ((value - minimum) / step).round() * step).clamp(minimum, maximum)
            }
        }
    }
}

/// What the user picked in the computation tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeTarget {
    Contribution(Contribution),
    MixPoint(MixPoint),
    /// A fixed leaf that is not a mix term. It can still expose its own input
    /// parameters, such as a style value used by the enclosing operation.
    Input(Input),
    Detector(Detector),
}

/// One leaf of a property computation tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    Band(BandComponent),
    /// The low four bands before the bass envelope smoothing.
    BassBands,
    Power,
    /// Raw broadband level before envelope smoothing.
    PowerLevel,
    Bass,
    /// Raw bass level before envelope smoothing.
    BassLevel,
    Kick,
    KickOnset,
    SnareOnset,
    DropOnset,
    /// The unwrapped shared animation clock used by the pitch oscillator.
    PhaseClock,
    Style(StyleInput),
    /// A value that is itself another property, used when one property feeds
    /// another (entrance feeds the pulse scale).
    Property(Property),
    Constant(&'static str),
}

impl Input {
    pub fn parameters(self) -> Vec<Parameter> {
        match self {
            Self::Band(component) => vec![
                Parameter::BandSource(component),
                Parameter::InputLow(EnvelopeTarget::Band(component)),
                Parameter::InputHigh(EnvelopeTarget::Band(component)),
                Parameter::Attack(EnvelopeTarget::Band(component)),
                Parameter::Release(EnvelopeTarget::Band(component)),
            ],
            Self::Power => vec![
                Parameter::InputLow(EnvelopeTarget::Power),
                Parameter::InputHigh(EnvelopeTarget::Power),
                Parameter::Attack(EnvelopeTarget::Power),
                Parameter::Release(EnvelopeTarget::Power),
            ],
            Self::Bass => vec![
                Parameter::InputLow(EnvelopeTarget::Bass),
                Parameter::InputHigh(EnvelopeTarget::Bass),
                Parameter::Attack(EnvelopeTarget::Bass),
                Parameter::Release(EnvelopeTarget::Bass),
            ],
            Self::Kick => vec![Parameter::KickDecay],
            Self::PhaseClock => vec![Parameter::Style(StyleInput::PhaseRate)],
            Self::Style(style) => vec![Parameter::Style(style)],
            Self::KickOnset
            | Self::SnareOnset
            | Self::DropOnset
            | Self::BassBands
            | Self::PowerLevel
            | Self::BassLevel
            | Self::Property(_)
            | Self::Constant(_) => Vec::new(),
        }
    }

    pub fn shape_target(self) -> Option<EnvelopeTarget> {
        match self {
            Self::Band(component) => Some(EnvelopeTarget::Band(component)),
            Self::Power => Some(EnvelopeTarget::Power),
            Self::Bass => Some(EnvelopeTarget::Bass),
            _ => None,
        }
    }

    pub fn detector(self) -> Option<Detector> {
        match self {
            Self::KickOnset => Some(Detector::Kick),
            Self::SnareOnset => Some(Detector::Snare),
            Self::DropOnset => Some(Detector::Drop),
            _ => None,
        }
    }

    pub fn title(self) -> String {
        match self {
            Self::Band(component) => format!("{} BAND", component.label()),
            Self::BassBands => "BASS BANDS".to_owned(),
            Self::Power => "PWR ENVELOPE".to_owned(),
            Self::PowerLevel => "PWR RAW LEVEL".to_owned(),
            Self::Bass => "BASS ENVELOPE".to_owned(),
            Self::BassLevel => "BASS RAW LEVEL".to_owned(),
            Self::Kick => "KICK ENVELOPE".to_owned(),
            Self::KickOnset => "KICK ONSET".to_owned(),
            Self::SnareOnset => "SNARE ONSET".to_owned(),
            Self::DropOnset => "DROP ONSET".to_owned(),
            Self::PhaseClock => "PHASE CLOCK".to_owned(),
            Self::Style(style) => Parameter::Style(style).label().to_owned(),
            Self::Property(property) => property.name().to_owned(),
            Self::Constant(value) => format!("CONSTANT {value}"),
        }
    }

    pub fn selectable(self) -> Option<SelectableInput> {
        Some(match self {
            Self::Band(component) => SelectableInput::Band(component),
            Self::BassBands => SelectableInput::BassBands,
            Self::Power => SelectableInput::Power,
            Self::PowerLevel => SelectableInput::PowerLevel,
            Self::Bass => SelectableInput::Bass,
            Self::BassLevel => SelectableInput::BassLevel,
            Self::Kick => SelectableInput::Kick,
            Self::KickOnset => SelectableInput::KickOnset,
            Self::SnareOnset => SelectableInput::SnareOnset,
            Self::DropOnset => SelectableInput::DropOnset,
            Self::PhaseClock => SelectableInput::PhaseClock,
            Self::Style(style) => SelectableInput::Style(style),
            Self::Property(_) | Self::Constant(_) => return None,
        })
    }

    pub fn selectable_index(self) -> Option<usize> {
        self.selectable().map(SelectableInput::index)
    }

    pub fn from_selectable(index: usize) -> Self {
        SelectableInput::from_index(index).input()
    }
}

/// An input that can be appended to a mix point. The persisted config keeps
/// this semantic form instead of the picker position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SelectableInput {
    Band(BandComponent),
    BassBands,
    Power,
    PowerLevel,
    Bass,
    BassLevel,
    Kick,
    KickOnset,
    SnareOnset,
    DropOnset,
    PhaseClock,
    Style(StyleInput),
}

impl SelectableInput {
    pub const ALL: [Self; 31] = [
        Self::Band(BandComponent::Spin),
        Self::Band(BandComponent::Pitch),
        Self::Band(BandComponent::Roll),
        Self::Band(BandComponent::Twist),
        Self::Band(BandComponent::Bulge),
        Self::Band(BandComponent::Ripple),
        Self::Band(BandComponent::Shear),
        Self::Band(BandComponent::Zoom),
        Self::Band(BandComponent::Accent),
        Self::Band(BandComponent::Line),
        Self::Band(BandComponent::Jitter),
        Self::Band(BandComponent::Phase),
        Self::BassBands,
        Self::Power,
        Self::PowerLevel,
        Self::Bass,
        Self::BassLevel,
        Self::Kick,
        Self::KickOnset,
        Self::SnareOnset,
        Self::DropOnset,
        Self::PhaseClock,
        Self::Style(StyleInput::AxisX),
        Self::Style(StyleInput::AxisY),
        Self::Style(StyleInput::AxisZ),
        Self::Style(StyleInput::PhaseRate),
        Self::Style(StyleInput::TwistSign),
        Self::Style(StyleInput::ShearSign),
        Self::Style(StyleInput::DeformBias),
        Self::Style(StyleInput::ZoomBias),
        Self::Style(StyleInput::JitterBias),
    ];

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|input| *input == self)
            .expect("every selectable input is listed")
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    pub fn input(self) -> Input {
        match self {
            Self::Band(component) => Input::Band(component),
            Self::BassBands => Input::BassBands,
            Self::Power => Input::Power,
            Self::PowerLevel => Input::PowerLevel,
            Self::Bass => Input::Bass,
            Self::BassLevel => Input::BassLevel,
            Self::Kick => Input::Kick,
            Self::KickOnset => Input::KickOnset,
            Self::SnareOnset => Input::SnareOnset,
            Self::DropOnset => Input::DropOnset,
            Self::PhaseClock => Input::PhaseClock,
            Self::Style(style) => Input::Style(style),
        }
    }
}

/// How one term enters its mix point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mix {
    Add,
    Mul,
}

impl Mix {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Add => Self::Mul,
            Self::Mul => Self::Add,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "ADD",
            Self::Mul => "MUL",
        }
    }
}

/// One editable aggregation in a property formula. A point has a primary
/// operation: in an additive point the multiplicative terms scale the sum, and
/// in a multiplicative point the additive terms offset the product. A pure sum
/// or product is therefore just a point whose terms all share the primary
/// operation.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    EnumCount,
    VariantArray,
    Serialize,
    Deserialize,
)]
pub enum MixPoint {
    PulseEntrance,
    PulseEnergy,
    PulseCamera,
    SpinSpeed,
    PitchTarget,
    PitchRate,
    RollTarget,
    RollRate,
    Twist,
    Bulge,
    Ripple,
    Shear,
    Jitter,
    Zoom,
    Accent,
    Line,
    PhaseSpeed,
    CrossfadeRate,
    MorphRate,
    EntranceRate,
}

/// One term fixed by a mix point definition.
#[derive(Clone, Copy, Debug)]
pub struct ContributionSpec {
    pub label: &'static str,
    /// Description of the term's own math, shown beside its mix badge.
    pub note: &'static str,
    pub source: Source,
    pub gain: Option<Gain>,
    /// A style value that scales the source inside this term.
    pub style_scale: Option<StyleInput>,
    pub mix: Mix,
}

impl ContributionSpec {
    pub fn input(self) -> Option<Input> {
        match self.source {
            Source::Input(input) => Some(input),
            Source::Smoothstep(input) => Some(input),
            Source::SinOscillator => Some(Input::PhaseClock),
            Source::Constant(_) => None,
        }
    }

    /// The built-in value of a constant term, before any runtime override.
    pub fn constant(self) -> Option<f32> {
        match self.source {
            Source::Constant(value) => Some(value),
            _ => None,
        }
    }
}

/// A term value before its gain and mix operation are applied.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    Input(Input),
    Constant(f32),
    Smoothstep(Input),
    SinOscillator,
}

/// A fixed or user-added term of a mix point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Contribution {
    Fixed(MixPoint, usize),
    Extra(ExtraRef),
}

/// Stable handle to an extra input. Indices would move when another extra is
/// removed, so extras keep a per-point serial number instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExtraRef {
    pub point: MixPoint,
    pub serial: u32,
}

/// One user-added input in a mix point.
#[derive(Clone, Copy, Debug)]
pub struct Extra {
    pub input: Input,
    pub gain: f32,
    pub mix: Mix,
}

/// Gain range and default of a user-added mix input.
const EXTRA_GAIN_MIN: f32 = 0.0;
const EXTRA_GAIN_MAX: f32 = 5.0;
const EXTRA_GAIN: f32 = 0.1;

/// Runtime overrides and user-added terms for every mix point.
#[derive(Clone, Debug, Default)]
pub struct Mixer {
    modes: BTreeMap<(MixPoint, usize), Mix>,
    constants: BTreeMap<(MixPoint, usize), f32>,
    extras: BTreeMap<MixPoint, Vec<(ExtraRef, Extra)>>,
    serials: BTreeMap<MixPoint, u32>,
}

impl Mixer {
    pub fn mode(&self, contribution: Contribution) -> Mix {
        match contribution {
            Contribution::Fixed(point, index) => self
                .modes
                .get(&(point, index))
                .copied()
                .unwrap_or_else(|| point.contributions()[index].mix),
            Contribution::Extra(extra) => self.extra(extra).mix,
        }
    }

    pub fn set_mode(&mut self, contribution: Contribution, mode: Mix) {
        match contribution {
            Contribution::Fixed(point, index) => {
                assert!(
                    index < point.contributions().len(),
                    "fixed contribution index must exist"
                );
                self.modes.insert((point, index), mode);
            }
            Contribution::Extra(extra) => self.extra_mut(extra).mix = mode,
        }
    }

    /// Current value of a constant term, including a runtime override. Input,
    /// oscillator, and extra terms have no adjustable constant value.
    pub fn constant_value(&self, contribution: Contribution) -> Option<f32> {
        let Contribution::Fixed(point, index) = contribution else {
            return None;
        };
        let default = point.contributions()[index].constant()?;
        Some(
            self.constants
                .get(&(point, index))
                .copied()
                .unwrap_or(default),
        )
    }

    pub fn set_constant_value(&mut self, contribution: Contribution, value: f32) {
        let Contribution::Fixed(point, index) = contribution else {
            panic!("extra inputs have no constant value");
        };
        assert!(
            point.contributions()[index].constant().is_some(),
            "only constant terms have an adjustable value"
        );
        self.constants.insert((point, index), value);
    }

    pub fn toggle_mode(&mut self, contribution: Contribution) -> Mix {
        let mode = self.mode(contribution).toggled();
        self.set_mode(contribution, mode);
        mode
    }

    pub fn extras(
        &self,
        point: MixPoint,
    ) -> impl DoubleEndedIterator<Item = (ExtraRef, &Extra)> + '_ {
        self.extras
            .get(&point)
            .into_iter()
            .flatten()
            .map(|(id, extra)| (*id, extra))
    }

    pub fn extra(&self, id: ExtraRef) -> &Extra {
        &self
            .extras
            .get(&id.point)
            .and_then(|extras| extras.iter().find(|(extra, _)| extra.serial == id.serial))
            .expect("extra input handles always refer to a live input")
            .1
    }

    fn extra_mut(&mut self, id: ExtraRef) -> &mut Extra {
        &mut self
            .extras
            .get_mut(&id.point)
            .and_then(|extras| {
                extras
                    .iter_mut()
                    .find(|(extra, _)| extra.serial == id.serial)
            })
            .expect("extra input handles always refer to a live input")
            .1
    }

    pub fn set_extra_input(&mut self, id: ExtraRef, input: Input) {
        assert!(
            input.selectable_index().is_some(),
            "extra inputs must be picked from the selectable list"
        );
        self.extra_mut(id).input = input;
    }

    pub fn set_extra_gain(&mut self, id: ExtraRef, gain: f32) {
        assert!(gain.is_finite(), "extra input gains must be finite");
        self.extra_mut(id).gain = gain;
    }

    pub fn add_extra(&mut self, point: MixPoint) -> ExtraRef {
        let serial = self.serials.entry(point).or_insert(0);
        let id = ExtraRef {
            point,
            serial: *serial,
        };
        *serial += 1;
        self.extras.entry(point).or_default().push((
            id,
            Extra {
                input: Input::Power,
                gain: EXTRA_GAIN,
                mix: Mix::Add,
            },
        ));
        id
    }

    pub fn remove_extra(&mut self, id: ExtraRef) {
        let extras = self
            .extras
            .get_mut(&id.point)
            .expect("extra input handles always refer to a live input");
        let index = extras
            .iter()
            .position(|(extra, _)| extra.serial == id.serial)
            .expect("extra input handles always refer to a live input");
        extras.remove(index);
        if extras.is_empty() {
            self.extras.remove(&id.point);
        }
    }

    pub(super) fn value(&self, point: MixPoint, context: &MixContext) -> f32 {
        let mut sum = 0.0;
        let mut summed = false;
        let mut product = 1.0;
        let mut multiplied = false;
        for (index, spec) in point.contributions().iter().enumerate() {
            let contribution = Contribution::Fixed(point, index);
            let constant = self.constants.get(&(point, index)).copied();
            let value = spec.value(context, constant);
            match self.mode(contribution) {
                Mix::Add => {
                    sum += value;
                    summed = true;
                }
                Mix::Mul => {
                    product *= value;
                    multiplied = true;
                }
            }
        }
        for extra in self.extras(point).map(|(_, extra)| extra) {
            let value = context.input(extra.input) * extra.gain;
            match extra.mix {
                Mix::Add => {
                    sum += value;
                    summed = true;
                }
                Mix::Mul => {
                    product *= value;
                    multiplied = true;
                }
            }
        }
        match point.primary() {
            Mix::Add if summed => sum * if multiplied { product } else { 1.0 },
            Mix::Add => product,
            Mix::Mul if multiplied => product + if summed { sum } else { 0.0 },
            Mix::Mul => sum,
        }
    }
}

/// Persisted mixer adjustments: term modes, constant values, and user-added
/// inputs. Anything absent keeps its built-in definition.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MixerTuning {
    modes: Vec<MixModeTuning>,
    constants: Vec<ConstantTuning>,
    extras: BTreeMap<MixPoint, Vec<ExtraTuning>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MixModeTuning {
    point: MixPoint,
    index: usize,
    mix: Mix,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConstantTuning {
    point: MixPoint,
    index: usize,
    value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtraTuning {
    input: SelectableInput,
    gain: f32,
    mix: Mix,
}

impl Mixer {
    /// The persisted part of this mixer. Extra serials are session handles and
    /// are reassigned when the tuning is applied.
    pub fn tuning(&self) -> MixerTuning {
        MixerTuning {
            modes: self
                .modes
                .iter()
                .map(|(&(point, index), &mix)| MixModeTuning { point, index, mix })
                .collect(),
            constants: self
                .constants
                .iter()
                .map(|(&(point, index), &value)| ConstantTuning {
                    point,
                    index,
                    value,
                })
                .collect(),
            extras: self
                .extras
                .iter()
                .map(|(&point, extras)| {
                    let extras = extras
                        .iter()
                        .map(|(_, extra)| ExtraTuning {
                            input: extra
                                .input
                                .selectable()
                                .expect("extra inputs are picked from the selectable list"),
                            gain: extra.gain,
                            mix: extra.mix,
                        })
                        .collect();
                    (point, extras)
                })
                .collect(),
        }
    }
}

impl MixerTuning {
    pub fn to_mixer(&self) -> anyhow::Result<Mixer> {
        let mut mixer = Mixer::default();
        let mut mode_keys = BTreeSet::new();
        for mode in &self.modes {
            Self::validate_fixed_index(mode.point, mode.index)?;
            anyhow::ensure!(
                mode_keys.insert((mode.point, mode.index)),
                "duplicate mix mode for {} term {}",
                mode.point.title(),
                mode.index
            );
            mixer.set_mode(Contribution::Fixed(mode.point, mode.index), mode.mix);
        }
        let mut constant_keys = BTreeSet::new();
        for constant in &self.constants {
            Self::validate_fixed_index(constant.point, constant.index)?;
            let contribution = Contribution::Fixed(constant.point, constant.index);
            anyhow::ensure!(
                constant.point.contributions()[constant.index]
                    .constant()
                    .is_some(),
                "{} term {} is not a constant",
                constant.point.title(),
                constant.index
            );
            anyhow::ensure!(
                constant_keys.insert((constant.point, constant.index)),
                "duplicate constant value for {} term {}",
                constant.point.title(),
                constant.index
            );
            validate_value(
                constant.value,
                Parameter::Constant(contribution).range(),
                "constant term value",
            )?;
            mixer.set_constant_value(contribution, constant.value);
        }
        for (&point, extras) in &self.extras {
            for extra in extras {
                validate_value(
                    extra.gain,
                    (EXTRA_GAIN_MIN, EXTRA_GAIN_MAX),
                    "extra input gain",
                )?;
                let id = mixer.add_extra(point);
                mixer.set_extra_input(id, extra.input.input());
                mixer.set_extra_gain(id, extra.gain);
                mixer.set_mode(Contribution::Extra(id), extra.mix);
            }
        }
        Ok(mixer)
    }

    fn validate_fixed_index(point: MixPoint, index: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            index < point.contributions().len(),
            "{} has no term {index}",
            point.title()
        );
        Ok(())
    }
}

impl ContributionSpec {
    fn value(self, context: &MixContext, constant_override: Option<f32>) -> f32 {
        let mut value = match self.source {
            Source::Input(input) => context.input(input),
            Source::Constant(default) => constant_override.unwrap_or(default),
            Source::Smoothstep(input) => super::smoothstep(context.input(input)),
            Source::SinOscillator => (context.time * 0.47 + context.phase).sin(),
        };
        if let Some(style) = self.style_scale {
            value *= style_value(&context.style, style);
        }
        if let Some(gain) = self.gain {
            value *= context.gains[gain];
        }
        value
    }
}

/// Values that every mix term can read. The context owns its copies so a mix
/// point does not borrow the model, the mixer, or the music simultaneously.
#[derive(Clone, Copy, Debug)]
pub(super) struct MixContext {
    pub bands: [f32; BAND_COUNT],
    pub raw_bands: [f32; BAND_COUNT],
    pub power: f32,
    pub power_level: f32,
    pub bass: f32,
    pub bass_level: f32,
    pub kick: f32,
    pub kick_onset: f32,
    pub snare_onset: f32,
    pub drop_onset: f32,
    pub gains: Gains,
    pub routing: BandRouting,
    pub style: Style,
    pub entrance: f32,
    pub phase: f32,
    pub time: f32,
}

impl MixContext {
    fn input(self, input: Input) -> f32 {
        match input {
            Input::Band(component) => self.bands[self.routing[component]],
            Input::BassBands => self.raw_bands[..4].iter().copied().fold(0.0_f32, f32::max),
            Input::Power => self.power,
            Input::PowerLevel => self.power_level,
            Input::Bass => self.bass,
            Input::BassLevel => self.bass_level,
            Input::Kick => self.kick,
            Input::KickOnset => self.kick_onset,
            Input::SnareOnset => self.snare_onset,
            Input::DropOnset => self.drop_onset,
            Input::PhaseClock => self.phase,
            Input::Style(style) => style_value(&self.style, style),
            Input::Property(Property::Entrance) => self.entrance,
            Input::Property(property) => {
                panic!("property input {} has no mix-time value", property.name())
            }
            Input::Constant(value) => {
                panic!("constant input {value} must be a mix point constant")
            }
        }
    }
}

const fn term(
    label: &'static str,
    note: &'static str,
    source: Source,
    gain: Option<Gain>,
    style_scale: Option<StyleInput>,
    mix: Mix,
) -> ContributionSpec {
    ContributionSpec {
        label,
        note,
        source,
        gain,
        style_scale,
        mix,
    }
}

const fn input_term(
    input: Input,
    label: &'static str,
    note: &'static str,
    gain: Option<Gain>,
    mix: Mix,
) -> ContributionSpec {
    term(label, note, Source::Input(input), gain, None, mix)
}

const fn constant_term(value: f32, label: &'static str) -> ContributionSpec {
    term(label, "", Source::Constant(value), None, None, Mix::Add)
}

static PULSE_ENTRANCE_TERMS: [ContributionSpec; 2] = [
    constant_term(0.62, "BASE"),
    term(
        "ENTRANCE",
        "smoothstep ×gain",
        Source::Smoothstep(Input::Property(Property::Entrance)),
        Some(Gain::PulseEntrance),
        None,
        Mix::Add,
    ),
];

static PULSE_ENERGY_TERMS: [ContributionSpec; 4] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::PulsePower),
        Mix::Add,
    ),
    input_term(
        Input::Bass,
        "BASS",
        "×gain; smoothed, max raw",
        Some(Gain::PulseBass),
        Mix::Add,
    ),
    input_term(
        Input::Kick,
        "KICK",
        "×gain",
        Some(Gain::PulseKick),
        Mix::Add,
    ),
    constant_term(0.9, "BASE"),
];

static PULSE_CAMERA_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Band(BandComponent::Zoom),
        "ZOOM BAND",
        "×gain",
        Some(Gain::ZoomBand),
        Mix::Add,
    ),
    constant_term(1.0, "BASE"),
];

static SPIN_SPEED_TERMS: [ContributionSpec; 4] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::SpinPower),
        Mix::Mul,
    ),
    input_term(
        Input::Band(BandComponent::Spin),
        "SPIN BAND",
        "×gain",
        Some(Gain::SpinBand),
        Mix::Add,
    ),
    constant_term(0.16, "BASE"),
    input_term(
        Input::Style(StyleInput::PhaseRate),
        "PHASE RATE",
        "×",
        None,
        Mix::Mul,
    ),
];

static PITCH_TARGET_TERMS: [ContributionSpec; 3] = [
    term(
        "PITCH BAND",
        "×gain ×AXIS Y",
        Source::Input(Input::Band(BandComponent::Pitch)),
        Some(Gain::PitchBand),
        Some(StyleInput::AxisY),
        Mix::Add,
    ),
    constant_term(-0.24, "BASE"),
    term(
        "OSCILLATOR",
        "sin(time×0.47+phase) ×gain",
        Source::SinOscillator,
        Some(Gain::PitchOscillator),
        None,
        Mix::Add,
    ),
];

static PITCH_RATE_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::PitchPowerRate),
        Mix::Add,
    ),
    constant_term(1.6, "BASE"),
];

static ROLL_TARGET_TERMS: [ContributionSpec; 1] = [term(
    "ROLL BAND",
    "×gain ×AXIS Z",
    Source::Input(Input::Band(BandComponent::Roll)),
    Some(Gain::RollBand),
    Some(StyleInput::AxisZ),
    Mix::Add,
)];

static ROLL_RATE_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::RollPowerRate),
        Mix::Add,
    ),
    constant_term(1.3, "BASE"),
];

static TWIST_TERMS: [ContributionSpec; 3] = [
    input_term(
        Input::Band(BandComponent::Twist),
        "TWIST BAND",
        "×gain",
        Some(Gain::TwistBand),
        Mix::Mul,
    ),
    input_term(
        Input::Style(StyleInput::DeformBias),
        "DEFORM BIAS",
        "×",
        None,
        Mix::Mul,
    ),
    input_term(
        Input::Style(StyleInput::TwistSign),
        "TWIST SIGN",
        "×",
        None,
        Mix::Mul,
    ),
];

static BULGE_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Band(BandComponent::Bulge),
        "BULGE BAND",
        "×gain",
        Some(Gain::BulgeBand),
        Mix::Mul,
    ),
    input_term(
        Input::Style(StyleInput::DeformBias),
        "DEFORM BIAS",
        "×",
        None,
        Mix::Mul,
    ),
];

static RIPPLE_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Band(BandComponent::Ripple),
        "RIPPLE BAND",
        "×gain",
        Some(Gain::RippleBand),
        Mix::Mul,
    ),
    input_term(
        Input::Style(StyleInput::DeformBias),
        "DEFORM BIAS",
        "×",
        None,
        Mix::Mul,
    ),
];

static SHEAR_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Band(BandComponent::Shear),
        "SHEAR BAND",
        "×gain",
        Some(Gain::ShearBand),
        Mix::Mul,
    ),
    input_term(
        Input::Style(StyleInput::ShearSign),
        "SHEAR SIGN",
        "×",
        None,
        Mix::Mul,
    ),
];

static JITTER_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Band(BandComponent::Jitter),
        "JITTER BAND",
        "×gain",
        Some(Gain::JitterBand),
        Mix::Mul,
    ),
    input_term(
        Input::Style(StyleInput::JitterBias),
        "JITTER BIAS",
        "×",
        None,
        Mix::Mul,
    ),
];

static ZOOM_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Band(BandComponent::Zoom),
        "ZOOM BAND",
        "×gain",
        Some(Gain::ZoomBand),
        Mix::Add,
    ),
    constant_term(1.0, "BASE"),
];

static ACCENT_TERMS: [ContributionSpec; 1] = [input_term(
    Input::Band(BandComponent::Accent),
    "BAND 12 ACCENT",
    "clamp 0..1; tint index at spawn",
    None,
    Mix::Add,
)];

static LINE_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Band(BandComponent::Line),
        "BAND 13 LINE",
        "×gain",
        Some(Gain::LineEnergy),
        Mix::Add,
    ),
    constant_term(LINE_ENERGY_BASE, "BASE"),
];

static PHASE_SPEED_TERMS: [ContributionSpec; 4] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::PhasePower),
        Mix::Add,
    ),
    input_term(
        Input::Band(BandComponent::Phase),
        "PHASE BAND",
        "×gain",
        Some(Gain::PhaseBand),
        Mix::Add,
    ),
    constant_term(0.35, "BASE"),
    input_term(
        Input::Style(StyleInput::PhaseRate),
        "PHASE RATE",
        "×",
        None,
        Mix::Mul,
    ),
];

static CROSSFADE_RATE_TERMS: [ContributionSpec; 3] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::XfadePower),
        Mix::Add,
    ),
    input_term(
        Input::Kick,
        "KICK",
        "×gain",
        Some(Gain::XfadeKick),
        Mix::Add,
    ),
    constant_term(1.2, "BASE"),
];

static MORPH_RATE_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::MorphPower),
        Mix::Add,
    ),
    constant_term(1.1, "BASE"),
];

static ENTRANCE_RATE_TERMS: [ContributionSpec; 2] = [
    input_term(
        Input::Power,
        "PWR",
        "×gain",
        Some(Gain::EntrancePower),
        Mix::Add,
    ),
    constant_term(1.4, "BASE"),
];

impl MixPoint {
    pub fn title(self) -> &'static str {
        match self {
            Self::PulseEntrance => "PULSE ENTRANCE",
            Self::PulseEnergy => "PULSE ENERGY",
            Self::PulseCamera => "PULSE CAMERA",
            Self::SpinSpeed => "SPIN SPEED",
            Self::PitchTarget => "PITCH TARGET",
            Self::PitchRate => "PITCH RATE",
            Self::RollTarget => "ROLL TARGET",
            Self::RollRate => "ROLL RATE",
            Self::Twist => "TWIST",
            Self::Bulge => "BULGE",
            Self::Ripple => "RIPPLE",
            Self::Shear => "SHEAR",
            Self::Jitter => "JITTER",
            Self::Zoom => "CAMERA ZOOM",
            Self::Accent => "ACCENT",
            Self::Line => "LINE ENERGY",
            Self::PhaseSpeed => "PHASE SPEED",
            Self::CrossfadeRate => "XFADE RATE",
            Self::MorphRate => "MORPH RATE",
            Self::EntranceRate => "ENTRANCE RATE",
        }
    }

    /// The operation that owns the plain sum or product shape of the point.
    pub fn primary(self) -> Mix {
        match self {
            Self::Twist | Self::Bulge | Self::Ripple | Self::Shear | Self::Jitter => Mix::Mul,
            _ => Mix::Add,
        }
    }

    pub fn contributions(self) -> &'static [ContributionSpec] {
        match self {
            Self::PulseEntrance => &PULSE_ENTRANCE_TERMS,
            Self::PulseEnergy => &PULSE_ENERGY_TERMS,
            Self::PulseCamera => &PULSE_CAMERA_TERMS,
            Self::SpinSpeed => &SPIN_SPEED_TERMS,
            Self::PitchTarget => &PITCH_TARGET_TERMS,
            Self::PitchRate => &PITCH_RATE_TERMS,
            Self::RollTarget => &ROLL_TARGET_TERMS,
            Self::RollRate => &ROLL_RATE_TERMS,
            Self::Twist => &TWIST_TERMS,
            Self::Bulge => &BULGE_TERMS,
            Self::Ripple => &RIPPLE_TERMS,
            Self::Shear => &SHEAR_TERMS,
            Self::Jitter => &JITTER_TERMS,
            Self::Zoom => &ZOOM_TERMS,
            Self::Accent => &ACCENT_TERMS,
            Self::Line => &LINE_TERMS,
            Self::PhaseSpeed => &PHASE_SPEED_TERMS,
            Self::CrossfadeRate => &CROSSFADE_RATE_TERMS,
            Self::MorphRate => &MORPH_RATE_TERMS,
            Self::EntranceRate => &ENTRANCE_RATE_TERMS,
        }
    }
}

/// One operation or input in a property computation tree.
#[derive(Clone, Copy, Debug)]
pub struct ComputeNode {
    pub label: &'static str,
    /// How this node's result enters its parent. The root leaves it empty.
    pub op: &'static str,
    pub input: Option<Input>,
    /// Set when this branch is an onset detector and can open the detector
    /// tuning pane.
    pub detector: Option<Detector>,
    /// Set when this branch is an editable mix point. Its contribution rows are
    /// generated from `MixPoint::contributions` and the mixer's extra inputs.
    pub mix_point: Option<MixPoint>,
    pub children: &'static [ComputeNode],
}

const fn branch(
    label: &'static str,
    op: &'static str,
    children: &'static [ComputeNode],
) -> ComputeNode {
    ComputeNode {
        label,
        op,
        input: None,
        detector: None,
        mix_point: None,
        children,
    }
}

const fn mix_branch(label: &'static str, point: MixPoint) -> ComputeNode {
    ComputeNode {
        label,
        op: "",
        input: None,
        detector: None,
        mix_point: Some(point),
        children: &[],
    }
}

const fn leaf(input: Input, label: &'static str, op: &'static str) -> ComputeNode {
    ComputeNode {
        label,
        op,
        input: Some(input),
        detector: None,
        mix_point: None,
        children: &[],
    }
}

const fn detector_branch(
    label: &'static str,
    op: &'static str,
    detector: Detector,
    children: &'static [ComputeNode],
) -> ComputeNode {
    ComputeNode {
        label,
        op,
        input: None,
        detector: Some(detector),
        mix_point: None,
        children,
    }
}

impl Property {
    /// The algebra behind the property, with each input's role written on the
    /// edge that feeds it into the parent operation. Mix point branches expand
    /// to their default contributions and the user's extra inputs in the
    /// inspector. Formulas and constants mirror `FigureRuntime::update`,
    /// `Scene::update`, and the detector bank.
    pub fn computation(self) -> &'static ComputeNode {
        match self {
            Self::Pulse => &PULSE,
            Self::Spin => &SPIN,
            Self::Pitch => &PITCH,
            Self::Roll => &ROLL,
            Self::Twist => &TWIST,
            Self::Bulge => &BULGE,
            Self::Ripple => &RIPPLE,
            Self::Shear => &SHEAR,
            Self::Zoom => &ZOOM,
            Self::Accent => &ACCENT,
            Self::Line => &LINE,
            Self::Jitter => &JITTER,
            Self::Phase => &PHASE,
            Self::Kick => &KICK,
            Self::Flip => &FLIP,
            Self::Quake => &QUAKE,
            Self::Crossfade => &CROSSFADE,
            Self::Morph => &MORPH,
            Self::Entrance => &ENTRANCE,
        }
    }

    pub fn mix_points(self) -> &'static [MixPoint] {
        match self {
            Self::Pulse => &[
                MixPoint::PulseEntrance,
                MixPoint::PulseEnergy,
                MixPoint::PulseCamera,
            ],
            Self::Spin => &[MixPoint::SpinSpeed],
            Self::Pitch => &[MixPoint::PitchTarget, MixPoint::PitchRate],
            Self::Roll => &[MixPoint::RollTarget, MixPoint::RollRate],
            Self::Twist => &[MixPoint::Twist],
            Self::Bulge => &[MixPoint::Bulge],
            Self::Ripple => &[MixPoint::Ripple],
            Self::Shear => &[MixPoint::Shear],
            Self::Zoom => &[MixPoint::Zoom],
            Self::Accent => &[MixPoint::Accent],
            Self::Line => &[MixPoint::Line],
            Self::Jitter => &[MixPoint::Jitter],
            Self::Phase => &[MixPoint::PhaseSpeed],
            Self::Kick => &[],
            Self::Flip => &[],
            Self::Quake => &[],
            Self::Crossfade => &[MixPoint::CrossfadeRate],
            Self::Morph => &[MixPoint::MorphRate],
            Self::Entrance => &[MixPoint::EntranceRate],
        }
    }
}

static PULSE: ComputeNode = branch(
    "PULSE = pose.scale",
    "",
    &[branch(
        "soft_saturate(1.45); max 0.35",
        "",
        &[branch(
            "PRODUCT",
            "×",
            &[
                mix_branch("ENTRANCE SCALE", MixPoint::PulseEntrance),
                mix_branch("ENERGY SUM", MixPoint::PulseEnergy),
                leaf(
                    Input::Style(StyleInput::ZoomBias),
                    "ZOOM BIAS",
                    "× fixed factor",
                ),
                mix_branch("CAMERA ZOOM", MixPoint::PulseCamera),
            ],
        )],
    )],
);

static SPIN: ComputeNode = branch(
    "SPIN speed (rad/s)",
    "",
    &[mix_branch("SUM × PHASE_RATE", MixPoint::SpinSpeed)],
);

static PITCH: ComputeNode = branch(
    "PITCH angle (smoothed)",
    "",
    &[branch(
        "smoother(target, rate)",
        "state",
        &[
            mix_branch("TARGET", MixPoint::PitchTarget),
            mix_branch("RATE", MixPoint::PitchRate),
        ],
    )],
);

static ROLL: ComputeNode = branch(
    "ROLL angle (smoothed)",
    "",
    &[branch(
        "smoother(target, rate)",
        "state",
        &[
            mix_branch("TARGET", MixPoint::RollTarget),
            mix_branch("RATE", MixPoint::RollRate),
        ],
    )],
);

static TWIST: ComputeNode = branch(
    "TWIST amount",
    "",
    &[mix_branch("BAND × BIAS × SIGN", MixPoint::Twist)],
);

static BULGE: ComputeNode = branch(
    "BULGE amount",
    "",
    &[mix_branch("BAND × BIAS", MixPoint::Bulge)],
);

static RIPPLE: ComputeNode = branch(
    "RIPPLE amount",
    "",
    &[mix_branch("BAND × BIAS", MixPoint::Ripple)],
);

static SHEAR: ComputeNode = branch(
    "SHEAR amount",
    "",
    &[mix_branch("BAND × SIGN", MixPoint::Shear)],
);

static ZOOM: ComputeNode = branch(
    "CAMERA ZOOM",
    "",
    &[mix_branch("1 + BAND × gain", MixPoint::Zoom)],
);

static ACCENT: ComputeNode = branch(
    "ACCENT tint selector",
    "",
    &[mix_branch("TINT INDEX", MixPoint::Accent)],
);

static LINE: ComputeNode = branch(
    "LINE energy",
    "",
    &[mix_branch("0.6 + gain × value", MixPoint::Line)],
);

static JITTER: ComputeNode = branch(
    "JITTER amount",
    "",
    &[mix_branch("BAND × BIAS", MixPoint::Jitter)],
);

static PHASE: ComputeNode = branch(
    "PHASE speed (rad/s)",
    "",
    &[mix_branch("SUM × PHASE_RATE", MixPoint::PhaseSpeed)],
);

static KICK: ComputeNode = branch(
    "KICK envelope",
    "",
    &[
        branch(
            "ON KICK: max(kick, onset power)",
            "state",
            &[detector_branch(
                "KICK ONSET",
                "kernel flux peak over adaptive threshold",
                Detector::Kick,
                &[leaf(Input::KickOnset, "DETECTOR", "TRIGGER")],
            )],
        ),
        branch("DECAY when idle", "kick ×= exp(-decay×dt)", &[]),
    ],
);

static FLIP: ComputeNode = branch(
    "FLIP pitch offset",
    "",
    &[
        branch(
            "every Nth snare: target += PI",
            "state",
            &[detector_branch(
                "SNARE ONSET",
                "kernel flux peak over adaptive threshold",
                Detector::Snare,
                &[
                    leaf(Input::SnareOnset, "DETECTOR", "TRIGGER"),
                    leaf(Input::Constant("N"), "FLIP EVERY", "snare count"),
                ],
            )],
        ),
        branch("damped spring", "oscillates, then settles", &[]),
    ],
);

static QUAKE: ComputeNode = branch(
    "QUAKE offset",
    "",
    &[
        branch(
            "velocity impulse",
            "state",
            &[
                detector_branch(
                    "KICK TRIGGER",
                    "sequenced H/V, L/R/T/B",
                    Detector::Kick,
                    &[leaf(Input::KickOnset, "DETECTOR", "TRIGGER")],
                ),
                detector_branch(
                    "DROP TRIGGER",
                    "always vertical",
                    Detector::Drop,
                    &[leaf(Input::DropOnset, "DETECTOR", "TRIGGER")],
                ),
            ],
        ),
        branch("damped spring", "oscillates around center", &[]),
    ],
);

static CROSSFADE: ComputeNode = branch(
    "XFADE progress",
    "",
    &[
        detector_branch(
            "DROP TRIGGER",
            "N low onsets during a bass gap; always changes the scene",
            Detector::Drop,
            &[leaf(Input::DropOnset, "DETECTOR", "TRIGGER")],
        ),
        mix_branch("RATE = 1.2 + gain×PWR + gain×KICK", MixPoint::CrossfadeRate),
    ],
);

static MORPH: ComputeNode = branch(
    "MORPH rate",
    "",
    &[mix_branch("1.1 + gain×PWR", MixPoint::MorphRate)],
);

static ENTRANCE: ComputeNode = branch(
    "ENTRANCE progress",
    "",
    &[branch(
        "+= dt × rate; max 1",
        "integrated",
        &[mix_branch("RATE", MixPoint::EntranceRate)],
    )],
);

#[cfg(test)]
mod tests {
    use super::*;
    use strum::{EnumCount, VariantArray};

    #[test]
    fn normalized_rescaling_stops_before_the_curve() {
        let shape = InputShape {
            low: 0.2,
            high: 0.8,
            curve: Curve::Inflated,
            amount: 2.0,
            tracking: ReferenceTracking::Manual,
        };
        assert!((shape.normalized(0.5) - 0.5).abs() < 1e-6);
        assert!((shape.apply(0.5) - 0.5_f32.powf(1.0 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn input_shapes_rescale_and_bend_before_the_envelope() {
        let linear = InputShape {
            low: 0.2,
            high: 0.8,
            curve: Curve::Linear,
            amount: 1.0,
            tracking: ReferenceTracking::Manual,
        };
        assert_eq!(linear.apply(0.1), 0.0);
        assert!((linear.apply(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(linear.apply(0.9), 1.0);

        let full = InputShape::default();
        let inflated = InputShape {
            curve: Curve::Inflated,
            ..full
        };
        assert!((inflated.apply(0.25) - 0.5).abs() < 1e-6);
        let deflated = InputShape {
            curve: Curve::Deflated,
            ..full
        };
        assert!((deflated.apply(0.5) - 0.25).abs() < 1e-6);
        let s_curve = InputShape {
            curve: Curve::SCurve,
            ..full
        };
        assert!((s_curve.apply(0.25) - 0.15625).abs() < 1e-6);
        let off = InputShape {
            curve: Curve::Inflated,
            amount: 0.0,
            ..full
        };
        assert!((off.apply(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn curves_keep_bending_past_the_named_amount() {
        let probe = 0.25;
        for curve in [Curve::Inflated, Curve::Deflated, Curve::SCurve] {
            let mut previous = 0.0;
            for amount in [1.0, 2.0, 4.0, 8.0] {
                let shape = InputShape {
                    curve,
                    amount,
                    ..InputShape::default()
                };
                assert_eq!(shape.apply(0.0), 0.0, "{} keeps its low end", curve.name());
                assert_eq!(shape.apply(1.0), 1.0, "{} keeps its high end", curve.name());
                let departure = (shape.apply(probe) - probe).abs();
                assert!(
                    departure > previous,
                    "{} at amount {amount} must bend further than at the previous amount",
                    curve.name()
                );
                previous = departure;
            }
        }
    }

    #[test]
    fn curves_stay_ordered_and_use_their_whole_range() {
        for curve in [
            Curve::Linear,
            Curve::Inflated,
            Curve::Deflated,
            Curve::SCurve,
        ] {
            for amount in [0.0, 0.5, 1.0, 2.0, 4.0, 8.0] {
                let shape = InputShape {
                    curve,
                    amount,
                    ..InputShape::default()
                };
                let mut previous = shape.apply(0.0);
                for step in 1..=100 {
                    let shaped = shape.apply(step as f32 / 100.0);
                    assert!(
                        shaped >= previous,
                        "{} at amount {amount} must never fall back",
                        curve.name()
                    );
                    assert!((0.0..=1.0).contains(&shaped));
                    previous = shaped;
                }
                assert!(
                    shape.apply(0.25) < shape.apply(0.5) && shape.apply(0.5) < shape.apply(0.75),
                    "{} at amount {amount} must not flatten to its bounds",
                    curve.name()
                );
            }
        }
    }

    #[test]
    fn every_property_has_a_non_empty_computation_tree() {
        for &property in Property::VARIANTS {
            let tree = property.computation();
            assert!(!tree.label.is_empty(), "{}", property.name());
            assert!(!tree.children.is_empty(), "{}", property.name());
        }
        assert_eq!(Property::VARIANTS.len(), Property::COUNT);
    }

    #[test]
    fn trees_show_how_analysis_inputs_enter_the_property() {
        let inputs = property_inputs(Property::Pulse);
        assert!(inputs.contains(&Input::Power));
        assert!(inputs.contains(&Input::Bass));
        assert!(inputs.contains(&Input::Band(BandComponent::Zoom)));
        assert!(inputs.contains(&Input::Property(Property::Entrance)));

        let inputs = property_inputs(Property::Kick);
        assert!(inputs.contains(&Input::KickOnset));

        let inputs = property_inputs(Property::Flip);
        assert!(inputs.contains(&Input::SnareOnset));

        let inputs = property_inputs(Property::Quake);
        assert!(inputs.contains(&Input::KickOnset));
        assert!(inputs.contains(&Input::DropOnset));

        let inputs = property_inputs(Property::Crossfade);
        assert!(inputs.contains(&Input::DropOnset));
        assert!(inputs.contains(&Input::Kick));
    }

    /// Inputs either appear as a mix term or as a condition leaf in the fixed
    /// part of the tree.
    fn property_inputs(property: Property) -> Vec<Input> {
        let mut inputs = Vec::new();
        collect_static_inputs(property.computation(), &mut inputs);
        for point in property.mix_points() {
            for spec in point.contributions() {
                if let Some(input) = spec.input() {
                    inputs.push(input);
                }
            }
        }
        inputs
    }

    fn collect_static_inputs(node: &ComputeNode, out: &mut Vec<Input>) {
        if let Some(input) = node.input {
            out.push(input);
        }
        for child in node.children {
            collect_static_inputs(child, out);
        }
    }

    #[test]
    fn component_gains_follow_the_tree_edges() {
        assert_eq!(
            Property::Pulse.component_gains(Input::Power),
            vec![Gain::PulsePower]
        );
        assert_eq!(
            Property::Pulse.component_gains(Input::Band(BandComponent::Zoom)),
            vec![Gain::ZoomBand]
        );
        assert_eq!(
            Property::Pitch.component_gains(Input::Style(StyleInput::AxisY)),
            vec![Gain::PitchBand]
        );
        assert!(Property::Line
            .component_gains(Input::Band(BandComponent::Line))
            .contains(&Gain::LineEnergy));
    }

    #[test]
    fn tree_analysis_leaves_match_the_graph_dependencies() {
        use std::collections::HashSet;
        for &property in Property::VARIANTS {
            let tree: HashSet<super::super::Dependency> = property_inputs(property)
                .into_iter()
                .flat_map(analysis_dependencies)
                .collect();
            let declared: HashSet<super::super::Dependency> =
                property.dependencies().iter().copied().collect();
            assert_eq!(tree, declared, "{}", property.name());
        }
    }

    fn analysis_dependencies(input: Input) -> Vec<super::super::Dependency> {
        use super::super::Dependency;
        match input {
            Input::Band(component) => vec![Dependency::Band(component)],
            Input::BassBands => vec![Dependency::BassBands],
            Input::Power => vec![Dependency::Power],
            // The bass envelope is read from the low bands, so a direct bass
            // term also depends on the raw bass band area.
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

    fn test_context(power: f32, bass: f32, kick: f32) -> MixContext {
        MixContext {
            bands: [0.5; BAND_COUNT],
            raw_bands: [0.5; BAND_COUNT],
            power,
            power_level: power,
            bass,
            bass_level: bass,
            kick,
            kick_onset: 0.5,
            snare_onset: 0.5,
            drop_onset: 0.5,
            gains: Gains::default(),
            routing: BandRouting::default(),
            style: Style::default(),
            entrance: 1.0,
            phase: 0.0,
            time: 0.0,
        }
    }

    #[test]
    fn mix_points_default_to_their_formula_terms() {
        let context = test_context(0.4, 0.3, 0.2);
        let gains = Gains::default();
        let mixer = Mixer::default();
        let expected = 0.9
            + gains[Gain::PulsePower] * context.power
            + gains[Gain::PulseBass] * context.bass
            + gains[Gain::PulseKick] * context.kick;
        assert!((mixer.value(MixPoint::PulseEnergy, &context) - expected).abs() < 1e-6);
    }

    #[test]
    fn constant_terms_accept_a_runtime_value() {
        let context = test_context(0.4, 0.3, 0.2);
        let point = MixPoint::PulseEnergy;
        let base = Contribution::Fixed(point, 3);
        assert_eq!(point.contributions()[3].label, "BASE");
        let mut mixer = Mixer::default();
        assert!((mixer.constant_value(base).unwrap() - 0.9).abs() < 1e-6);
        let before = mixer.value(point, &context);
        mixer.set_constant_value(base, 0.4);
        let after = mixer.value(point, &context);
        assert!((before - after - 0.5).abs() < 1e-6);
        assert_eq!(mixer.constant_value(base), Some(0.4));
        assert_eq!(mixer.constant_value(Contribution::Fixed(point, 0)), None);
    }

    #[test]
    fn changing_a_term_to_mul_scales_the_additive_part() {
        let context = test_context(0.4, 0.3, 0.2);
        let gains = Gains::default();
        let mut mixer = Mixer::default();
        let point = MixPoint::PulseEnergy;
        let before = mixer.value(point, &context);
        mixer.set_mode(Contribution::Fixed(point, 0), Mix::Mul);
        let after = mixer.value(point, &context);
        let factor = gains[Gain::PulsePower] * context.power;
        let additive =
            0.9 + gains[Gain::PulseBass] * context.bass + gains[Gain::PulseKick] * context.kick;
        assert!((after - additive * factor).abs() < 1e-6);
        assert!(after < before);
    }

    #[test]
    fn extra_inputs_add_to_a_mix_point_and_can_be_removed() {
        let context = test_context(0.4, 0.3, 0.2);
        let mut mixer = Mixer::default();
        let point = MixPoint::PulseEnergy;
        let before = mixer.value(point, &context);
        let extra = mixer.add_extra(point);
        mixer.set_extra_input(extra, Input::Power);
        mixer.set_extra_gain(extra, 0.5);
        let after = mixer.value(point, &context);
        assert!((after - (before + 0.5 * context.power)).abs() < 1e-6);
        mixer.set_mode(Contribution::Extra(extra), Mix::Mul);
        assert!(mixer.mode(Contribution::Extra(extra)) == Mix::Mul);
        mixer.remove_extra(extra);
        assert_eq!(mixer.value(point, &context), before);
    }

    #[test]
    fn extra_handles_stay_valid_when_another_input_is_removed() {
        let point = MixPoint::SpinSpeed;
        let mut mixer = Mixer::default();
        let first = mixer.add_extra(point);
        let second = mixer.add_extra(point);
        mixer.remove_extra(first);
        assert_eq!(mixer.extra(second).input, Input::Power);
        let third = mixer.add_extra(point);
        assert_ne!(third, second);
        assert_eq!(
            mixer.extras(point).map(|(id, _)| id).collect::<Vec<_>>(),
            vec![second, third]
        );
    }

    #[test]
    fn adaptive_refs_fall_slowly_and_rise_quickly() {
        let mut shape = InputShape {
            high: 0.8,
            low: 0.2,
            ..InputShape::default()
        };
        shape.tracking = ReferenceTracking::Adaptive;
        let dt = 1.0 / 60.0;
        let high_start = shape.high;
        let low_start = shape.low;
        for _ in 0..30 {
            shape.observe(0.05, dt);
        }
        assert!(
            shape.high < high_start,
            "the high ref falls toward quiet input"
        );
        assert!(
            shape.high > 0.05,
            "the high ref falls slowly instead of following the value"
        );
        assert!(shape.low < low_start, "quiet input weighs the low ref down");
        let quiet_high = shape.high;
        for _ in 0..8 {
            shape.observe(1.0, dt);
        }
        assert!(
            shape.high > quiet_high + 0.1,
            "a peak bumps the high ref up quickly"
        );
        assert!(shape.low < shape.high);
        assert!(shape.observe(0.5, dt).is_finite());
    }

    #[test]
    fn every_mix_point_has_terms_and_belongs_to_one_property() {
        let mut owners = Vec::new();
        for &property in Property::VARIANTS {
            for &point in property.mix_points() {
                owners.push(point);
            }
        }
        assert_eq!(owners.len(), MixPoint::COUNT);
        for point in MixPoint::VARIANTS {
            assert!(!point.contributions().is_empty(), "{point:?}");
            assert_eq!(
                owners.iter().filter(|owner| **owner == *point).count(),
                1,
                "{point:?}"
            );
        }
    }

    #[test]
    fn the_tree_exposes_exactly_the_property_mix_points() {
        for &property in Property::VARIANTS {
            let mut tree_points = Vec::new();
            collect_tree_points(property.computation(), &mut tree_points);
            assert_eq!(tree_points, property.mix_points(), "{}", property.name());
        }
    }

    fn collect_tree_points(node: &ComputeNode, out: &mut Vec<MixPoint>) {
        if let Some(point) = node.mix_point {
            out.push(point);
        }
        for child in node.children {
            collect_tree_points(child, out);
        }
    }

    #[test]
    fn selectable_extra_inputs_are_unique() {
        for (index, input) in SelectableInput::ALL.iter().enumerate() {
            assert_eq!(input.index(), index);
            assert_eq!(input.input().selectable(), Some(*input));
            assert_eq!(
                SelectableInput::ALL
                    .iter()
                    .filter(|candidate| *candidate == input)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn mixer_tuning_round_trips_overrides_and_extras() {
        let point = MixPoint::PulseEnergy;
        let base = Contribution::Fixed(point, 3);
        let mut mixer = Mixer::default();
        mixer.set_mode(Contribution::Fixed(point, 0), Mix::Mul);
        mixer.set_constant_value(base, 0.4);
        let extra = mixer.add_extra(MixPoint::SpinSpeed);
        mixer.set_extra_input(extra, Input::Band(BandComponent::Bulge));
        mixer.set_extra_gain(extra, 1.5);
        mixer.set_mode(Contribution::Extra(extra), Mix::Mul);

        let tuning = mixer.tuning();
        let restored = tuning.to_mixer().unwrap();
        assert_eq!(restored.tuning(), tuning);
        assert_eq!(restored.mode(Contribution::Fixed(point, 0)), Mix::Mul);
        assert_eq!(restored.constant_value(base), Some(0.4));
        let restored_extra = restored
            .extras(MixPoint::SpinSpeed)
            .next()
            .expect("the extra input survives the round trip");
        assert_eq!(restored_extra.1.input, Input::Band(BandComponent::Bulge));
        assert_eq!(restored_extra.1.gain, 1.5);
        assert_eq!(restored_extra.1.mix, Mix::Mul);
    }

    #[test]
    fn mixer_tuning_rejects_unknown_fixed_terms() {
        let mut tuning = MixerTuning::default();
        tuning.modes.push(MixModeTuning {
            point: MixPoint::PulseEnergy,
            index: 99,
            mix: Mix::Mul,
        });
        assert!(tuning.to_mixer().is_err());
    }
}
