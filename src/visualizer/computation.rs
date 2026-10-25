use strum::{EnumCount, VariantArray};

use super::Property;

/// One non-analysis value that participates in a property computation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    Bass,
    Wide,
}

impl Detector {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bass => "BASS ONSET",
            Self::Wide => "WIDE ONSET",
        }
    }

    pub fn parameters(self) -> [Parameter; 4] {
        [
            Parameter::OnsetThreshold(self),
            Parameter::OnsetMinFlux(self),
            Parameter::OnsetBaselineRatio(self),
            Parameter::OnsetCooldown(self),
        ]
    }
}

/// A semantic band input. The actual FFT band is routed by `BandRouting`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, strum::EnumCount, strum::VariantArray)]
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
            Self::Spin => 4,
            Self::Pitch => 5,
            Self::Roll => 6,
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
#[derive(Clone, Copy, Debug)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnvelopeTarget {
    Band(BandComponent),
    Power,
    Bass,
    Treble,
}

/// One multiplier attached to an incoming component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, strum::EnumCount, strum::VariantArray)]
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
    KickFlux,
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
            Self::SpinBand => 0.7,
            Self::PitchBand => 0.55,
            Self::PitchOscillator => 0.08,
            Self::PitchPowerRate => 1.8,
            Self::RollBand => 0.9,
            Self::RollPowerRate => 1.6,
            Self::TwistBand => 1.7,
            Self::BulgeBand => 0.32,
            Self::RippleBand => 0.75,
            Self::ShearBand => 0.55,
            Self::LineEnergy => 0.4,
            Self::JitterBand => 0.055,
            Self::PhasePower => 2.6,
            Self::PhaseBand => 0.7,
            Self::KickFlux => 2.0,
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
            Self::KickFlux => "FLUX GAIN",
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

#[derive(Clone, Copy, Debug)]
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

/// One adjustable scalar in a computation tree.
/// Cheap pre-envelope response curves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Curve {
    Linear,
    Inflated,
    Deflated,
    SCurve,
}

impl Curve {
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

    fn apply(self, value: f32, amount: f32) -> f32 {
        let amount = amount.clamp(0.0, 8.0);
        let shaped = match self {
            Self::Linear => value,
            Self::Inflated => value.sqrt(),
            Self::Deflated => value * value,
            Self::SCurve => value * value * (3.0 - 2.0 * value),
        };
        value + (shaped - value) * amount
    }
}

/// Rescales a raw analysis value into 0..1 and optionally bends it before the
/// envelope smoothing stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputShape {
    pub low: f32,
    pub high: f32,
    pub curve: Curve,
    pub amount: f32,
}

impl Default for InputShape {
    fn default() -> Self {
        Self {
            low: 0.0,
            high: 1.0,
            curve: Curve::Linear,
            amount: 1.0,
        }
    }
}

impl InputShape {
    pub fn apply(self, value: f32) -> f32 {
        assert!(
            self.low < self.high,
            "input shape reference points must be ordered"
        );
        let normalized = ((value - self.low) / (self.high - self.low)).clamp(0.0, 1.0);
        self.curve.apply(normalized, self.amount).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Parameter {
    OnsetThreshold(Detector),
    OnsetMinFlux(Detector),
    OnsetBaselineRatio(Detector),
    OnsetCooldown(Detector),
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
}

impl Parameter {
    pub fn label(self) -> &'static str {
        match self {
            Self::OnsetThreshold(_) => "THRESHOLD",
            Self::OnsetMinFlux(_) => "MIN FLUX",
            Self::OnsetBaselineRatio(_) => "BASELINE x",
            Self::OnsetCooldown(_) => "COOLDOWN s",
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
        }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            Self::OnsetThreshold(_) => (0.0, 1.0),
            Self::OnsetMinFlux(_) => (0.0, 0.2),
            Self::OnsetBaselineRatio(_) => (1.0, 2.0),
            Self::OnsetCooldown(_) => (0.0, 1.0),
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
            Self::BandSource(_) => (0.0, (super::BAND_COUNT - 1) as f32),
            Self::InputLow(_) => (0.0, 0.99),
            Self::InputHigh(_) => (0.01, 1.0),
            Self::InputCurve(_) => (0.0, 3.0),
            Self::InputAmount(_) => (0.0, 8.0),
        }
    }

    pub fn step(self) -> f32 {
        match self {
            Self::OnsetThreshold(_) | Self::OnsetBaselineRatio(_) | Self::OnsetCooldown(_) => 0.01,
            Self::OnsetMinFlux(_) => 0.001,
            Self::Attack(_) | Self::Release(_) => 0.5,
            Self::KickDecay => 0.1,
            Self::Style(_) => 0.01,
            Self::Gain(gain) => gain.step(),
            Self::BandSource(_) | Self::InputCurve(_) => 1.0,
            Self::InputLow(_) | Self::InputHigh(_) | Self::InputAmount(_) => 0.01,
        }
    }

    pub fn format(self, value: f32) -> String {
        match self {
            Self::OnsetMinFlux(_) | Self::Gain(Gain::JitterBand) => format!("{value:.3}"),
            Self::Attack(_) | Self::Release(_) => format!("{value:.1}"),
            Self::BandSource(_) => format!("B{:02}", value.round() as usize),
            Self::InputCurve(_) => Curve::from_index(value.round() as usize).name().to_owned(),
            _ => format!("{value:.2}"),
        }
    }

    /// Slider position for a value, using a logarithmic scale for envelope
    /// times so the low end gains more travel.
    pub fn fraction(self, value: f32) -> f32 {
        let (minimum, maximum) = self.range();
        let value = value.clamp(minimum, maximum);
        match self {
            Self::Attack(_) | Self::Release(_) => (value / minimum).ln() / (maximum / minimum).ln(),
            Self::BandSource(_) => (value - minimum) / (maximum - minimum),
            _ => (value - minimum) / (maximum - minimum),
        }
    }

    pub fn from_fraction(self, fraction: f32) -> f32 {
        let (minimum, maximum) = self.range();
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            Self::Attack(_) | Self::Release(_) => {
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
    Component(Input),
    Detector(Detector),
}

/// One leaf of a property computation tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    Band(BandComponent),
    /// The low four bands before the bass envelope smoothing.
    BassBands,
    Power,
    /// Raw broadband level before envelope smoothing, used by the wide onset.
    PowerLevel,
    Bass,
    /// Raw bass level before envelope smoothing, used by the bass onset.
    BassLevel,
    BassFlux,
    Flux,
    Kick,
    BassOnset,
    WideOnset,
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
            Self::BassOnset
            | Self::WideOnset
            | Self::BassBands
            | Self::PowerLevel
            | Self::BassLevel
            | Self::BassFlux
            | Self::Flux
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
            Self::BassOnset => Some(Detector::Bass),
            Self::WideOnset => Some(Detector::Wide),
            _ => None,
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
        children,
    }
}

const fn leaf(input: Input, label: &'static str, op: &'static str) -> ComputeNode {
    ComputeNode {
        label,
        op,
        input: Some(input),
        detector: None,
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
        children,
    }
}

impl Property {
    /// The algebra behind the property, with each input's role written on the
    /// edge that feeds it into the parent operation. This is the tree shown by
    /// the inspector; formulas and constants mirror `FigureRuntime::update`,
    /// `Scene::update`, and `Onset::observe`.
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
            Self::Crossfade => &CROSSFADE,
            Self::Morph => &MORPH,
            Self::Entrance => &ENTRANCE,
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
                branch(
                    "ENTRANCE SCALE",
                    "×",
                    &[leaf(
                        Input::Property(Property::Entrance),
                        "ENTRANCE",
                        "smoothstep ×gain +0.62",
                    )],
                ),
                branch(
                    "ENERGY SUM",
                    "×",
                    &[
                        leaf(Input::Power, "PWR", "×gain add"),
                        branch(
                            "BASS ENVELOPE",
                            "×gain add",
                            &[
                                leaf(Input::Bass, "SMOOTHED", "attack / release"),
                                leaf(Input::BassBands, "BANDS 00-03 RAW", "max, then smooth"),
                            ],
                        ),
                        leaf(Input::Kick, "KICK", "×gain add"),
                        leaf(Input::Constant("+0.9"), "BASE", "add"),
                    ],
                ),
                leaf(Input::Style(StyleInput::ZoomBias), "ZOOM BIAS", "×"),
                branch(
                    "CAMERA ZOOM",
                    "×",
                    &[
                        leaf(Input::Band(BandComponent::Zoom), "ZOOM BAND", "×gain add"),
                        leaf(Input::Constant("+1.0"), "BASE", "add"),
                    ],
                ),
            ],
        )],
    )],
);

static SPIN: ComputeNode = branch(
    "SPIN speed (rad/s)",
    "",
    &[branch(
        "SUM × PHASE_RATE",
        "",
        &[
            branch(
                "SUM",
                "×",
                &[
                    leaf(Input::Power, "PWR", "×gain add"),
                    leaf(Input::Band(BandComponent::Spin), "SPIN BAND", "×gain add"),
                    leaf(Input::Constant("+0.16"), "BASE", "add"),
                ],
            ),
            leaf(Input::Style(StyleInput::PhaseRate), "PHASE RATE", "×"),
        ],
    )],
);

static PITCH: ComputeNode = branch(
    "PITCH angle (smoothed)",
    "",
    &[branch(
        "smoother(target, rate)",
        "state",
        &[
            branch(
                "TARGET",
                "approach",
                &[
                    leaf(
                        Input::Band(BandComponent::Pitch),
                        "PITCH BAND",
                        "×gain ×AXIS_Y add",
                    ),
                    leaf(Input::Style(StyleInput::AxisY), "AXIS Y", "×band ×gain"),
                    leaf(Input::Constant("-0.24"), "BASE", "add"),
                    branch(
                        "OSCILLATOR",
                        "add ×gain",
                        &[leaf(Input::PhaseClock, "PHASE", "sin(time ×0.47 + phase)")],
                    ),
                ],
            ),
            leaf(Input::Power, "PWR", "smoothing rate ×gain add"),
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
            branch(
                "TARGET",
                "approach",
                &[
                    leaf(
                        Input::Band(BandComponent::Roll),
                        "ROLL BAND",
                        "×gain ×AXIS_Z",
                    ),
                    leaf(Input::Style(StyleInput::AxisZ), "AXIS Z", "×band ×gain"),
                ],
            ),
            leaf(Input::Power, "PWR", "smoothing rate ×gain add"),
        ],
    )],
);

static TWIST: ComputeNode = branch(
    "TWIST amount",
    "",
    &[branch(
        "BAND × BIAS × SIGN",
        "",
        &[
            leaf(Input::Band(BandComponent::Twist), "TWIST BAND", "×gain"),
            leaf(Input::Style(StyleInput::DeformBias), "DEFORM BIAS", "×"),
            leaf(Input::Style(StyleInput::TwistSign), "TWIST SIGN", "×"),
        ],
    )],
);

static BULGE: ComputeNode = branch(
    "BULGE amount",
    "",
    &[branch(
        "BAND × BIAS",
        "",
        &[
            leaf(Input::Band(BandComponent::Bulge), "BULGE BAND", "×gain"),
            leaf(Input::Style(StyleInput::DeformBias), "DEFORM BIAS", "×"),
        ],
    )],
);

static RIPPLE: ComputeNode = branch(
    "RIPPLE amount",
    "",
    &[branch(
        "BAND × BIAS",
        "",
        &[
            leaf(Input::Band(BandComponent::Ripple), "RIPPLE BAND", "×gain"),
            leaf(Input::Style(StyleInput::DeformBias), "DEFORM BIAS", "×"),
        ],
    )],
);

static SHEAR: ComputeNode = branch(
    "SHEAR amount",
    "",
    &[branch(
        "BAND × SIGN",
        "",
        &[
            leaf(Input::Band(BandComponent::Shear), "SHEAR BAND", "×gain"),
            leaf(Input::Style(StyleInput::ShearSign), "SHEAR SIGN", "×"),
        ],
    )],
);

static ZOOM: ComputeNode = branch(
    "CAMERA ZOOM",
    "",
    &[branch(
        "1 + BAND × gain",
        "",
        &[
            leaf(Input::Band(BandComponent::Zoom), "ZOOM BAND", "×gain add"),
            leaf(Input::Constant("+1.0"), "BASE", "add"),
        ],
    )],
);

static ACCENT: ComputeNode = branch(
    "ACCENT tint selector",
    "",
    &[leaf(
        Input::Band(BandComponent::Accent),
        "BAND 12 ACCENT",
        "clamp 0..1; tint index at spawn",
    )],
);

static LINE: ComputeNode = branch(
    "LINE energy",
    "",
    &[leaf(
        Input::Band(BandComponent::Line),
        "BAND 13 LINE",
        "0.6 + gain × value; max NEAR",
    )],
);

static JITTER: ComputeNode = branch(
    "JITTER amount",
    "",
    &[branch(
        "BAND × BIAS",
        "",
        &[
            leaf(Input::Band(BandComponent::Jitter), "JITTER BAND", "×gain"),
            leaf(Input::Style(StyleInput::JitterBias), "JITTER BIAS", "×"),
        ],
    )],
);

static PHASE: ComputeNode = branch(
    "PHASE speed (rad/s)",
    "",
    &[branch(
        "SUM × PHASE_RATE",
        "",
        &[
            branch(
                "SUM",
                "×",
                &[
                    leaf(Input::Power, "PWR", "×gain add"),
                    leaf(Input::Band(BandComponent::Phase), "PHASE BAND", "×gain add"),
                    leaf(Input::Constant("+0.35"), "BASE", "add"),
                ],
            ),
            leaf(Input::Style(StyleInput::PhaseRate), "PHASE RATE", "×"),
        ],
    )],
);

static KICK: ComputeNode = branch(
    "KICK envelope",
    "",
    &[
        branch(
            "ON HIT: max(kick, strength)",
            "state",
            &[
                detector_branch(
                    "BASS ONSET SWITCH",
                    "if all true",
                    Detector::Bass,
                    &[
                        leaf(Input::BassOnset, "DETECTOR", "ARMED / IDLE"),
                        branch(
                            "BASS level",
                            "≥ THR; ≥ baseline×ratio+.015",
                            &[
                                leaf(Input::BassLevel, "RAW", "level test"),
                                leaf(Input::BassBands, "BANDS 00-03 RAW", "max"),
                            ],
                        ),
                        leaf(Input::BassFlux, "BASS flux", "≥ min flux"),
                        leaf(Input::Constant("== 0"), "ONSET COOLDOWN", "must hold"),
                    ],
                ),
                branch(
                    "STRENGTH clamp(level+gain×flux,.25,1.5)",
                    "max(kick, strength)",
                    &[
                        leaf(Input::BassLevel, "BASS level", "add"),
                        leaf(Input::BassFlux, "BASS flux", "×gain add"),
                    ],
                ),
            ],
        ),
        branch("DECAY when idle", "kick ×= exp(-decay×dt)", &[]),
    ],
);

static CROSSFADE: ComputeNode = branch(
    "XFADE progress",
    "",
    &[
        branch(
            "HIT TRIGGER",
            "retarget scene / params",
            &[
                detector_branch(
                    "BASS ONSET SWITCH",
                    "55% scene, else 85% param",
                    Detector::Bass,
                    &[
                        leaf(Input::BassOnset, "DETECTOR", "ARMED / IDLE"),
                        leaf(
                            Input::BassLevel,
                            "BASS level",
                            "≥ THR; ≥ baseline×ratio+.015",
                        ),
                        leaf(Input::BassFlux, "BASS flux", "≥ min flux"),
                    ],
                ),
                detector_branch(
                    "WIDE ONSET SWITCH",
                    "18% scene, else 45% param",
                    Detector::Wide,
                    &[
                        leaf(Input::WideOnset, "DETECTOR", "ARMED / IDLE"),
                        leaf(
                            Input::PowerLevel,
                            "PWR level",
                            "≥ THR; ≥ baseline×ratio+.015",
                        ),
                        leaf(Input::Flux, "FLUX", "≥ min flux"),
                    ],
                ),
                leaf(
                    Input::Constant("transition cooldown == 0"),
                    "HIT GATE",
                    "must hold",
                ),
            ],
        ),
        branch(
            "RATE = 1.2 + gain×PWR + gain×KICK",
            "transition += dt × rate",
            &[
                leaf(Input::Power, "PWR", "×gain add"),
                leaf(Input::Kick, "KICK", "×gain add"),
            ],
        ),
    ],
);

static MORPH: ComputeNode = branch(
    "MORPH rate",
    "",
    &[branch(
        "1.1 + gain×PWR",
        "",
        &[
            leaf(Input::Power, "PWR", "×gain add"),
            leaf(Input::Constant("+1.1"), "BASE", "add"),
        ],
    )],
);

static ENTRANCE: ComputeNode = branch(
    "ENTRANCE progress",
    "",
    &[branch(
        "+= dt × (1.4 + gain×PWR); max 1",
        "integrated",
        &[
            leaf(Input::Power, "PWR", "rate ×gain add"),
            leaf(Input::Constant("+1.4"), "BASE", "add"),
        ],
    )],
);

#[cfg(test)]
mod tests {
    use super::*;
    use strum::{EnumCount, VariantArray};

    #[test]
    fn input_shapes_rescale_and_bend_before_the_envelope() {
        let linear = InputShape {
            low: 0.2,
            high: 0.8,
            curve: Curve::Linear,
            amount: 1.0,
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
        let pulse = Property::Pulse.computation();
        let mut inputs = Vec::new();
        collect_inputs(pulse, &mut inputs);
        assert!(inputs.contains(&Input::Power));
        assert!(inputs.contains(&Input::BassBands));
        assert!(inputs.contains(&Input::Band(BandComponent::Zoom)));
        assert!(inputs.contains(&Input::Property(Property::Entrance)));

        let kick = Property::Kick.computation();
        inputs.clear();
        collect_inputs(kick, &mut inputs);
        assert!(inputs.contains(&Input::BassOnset));
        assert!(inputs.contains(&Input::BassFlux));

        let crossfade = Property::Crossfade.computation();
        inputs.clear();
        collect_inputs(crossfade, &mut inputs);
        assert!(inputs.contains(&Input::BassOnset));
        assert!(inputs.contains(&Input::WideOnset));
        assert!(inputs.contains(&Input::Kick));
    }

    fn collect_inputs(node: &ComputeNode, out: &mut Vec<Input>) {
        if let Some(input) = node.input {
            out.push(input);
        }
        for child in node.children {
            collect_inputs(child, out);
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
        assert_eq!(
            Property::Kick.component_gains(Input::BassFlux),
            vec![Gain::KickFlux]
        );
        assert!(Property::Line
            .component_gains(Input::Band(BandComponent::Line))
            .contains(&Gain::LineEnergy));
    }

    #[test]
    fn tree_analysis_leaves_match_the_graph_dependencies() {
        use std::collections::HashSet;
        for &property in Property::VARIANTS {
            let mut inputs = Vec::new();
            collect_inputs(property.computation(), &mut inputs);
            let tree: HashSet<super::super::Dependency> =
                inputs.into_iter().filter_map(analysis_dependency).collect();
            let declared: HashSet<super::super::Dependency> =
                property.dependencies().iter().copied().collect();
            assert_eq!(tree, declared, "{}", property.name());
        }
    }

    fn analysis_dependency(input: Input) -> Option<super::super::Dependency> {
        use super::super::Dependency;
        match input {
            Input::Band(component) => Some(Dependency::Band(component)),
            Input::BassBands => Some(Dependency::BassBands),
            Input::Power => Some(Dependency::Power),
            Input::Bass => Some(Dependency::Bass),
            Input::Kick => Some(Dependency::Kick),
            Input::BassOnset => Some(Dependency::BassOnset),
            Input::WideOnset => Some(Dependency::WideOnset),
            Input::PowerLevel
            | Input::BassLevel
            | Input::BassFlux
            | Input::Flux
            | Input::PhaseClock
            | Input::Style(_)
            | Input::Property(_)
            | Input::Constant(_) => None,
        }
    }
}
