//! Per-band relative-excess extraction and the kick, snare, hi-hat, and drop detectors.
//!
//! Every band extracts a continuous onset signal from its FFT bins. Each bin
//! keeps a background envelope, and the positive relative excess over that
//! background is capped and averaged over the band. ABDR, WSDR, DRP, and ABL
//! then position that signal relative to the whole spectrum before the composite
//! detectors multiply it by their kernels.
//!
//! Only the kick, snare, hi-hat, and drop detectors produce boolean onsets, from the
//! kernel-projected continuous flow. The drop gates its onset detector with a
//! smoothed RMS envelope below a configurable dBFS threshold.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::spectrum::{RelativeExcessSettings, Spectrum};
use super::{Curve, BAND_COUNT};

pub const ABL_GAIN_MIN: f32 = 0.5;
pub const ABL_GAIN_MAX: f32 = 8.0;
pub const ABL_AMOUNT_MIN: f32 = 0.0;
pub const EXCESS_ATTACK_MIN: f32 = 0.05;
pub const EXCESS_ATTACK_MAX: f32 = 2.0;
pub const EXCESS_RELEASE_MIN: f32 = 0.05;
pub const EXCESS_RELEASE_MAX: f32 = 2.0;
pub const EXCESS_FLOOR_MIN: f32 = 0.0;
pub const EXCESS_FLOOR_MAX: f32 = 0.2;
pub const EXCESS_CAP_MIN: f32 = 0.1;
pub const EXCESS_CAP_MAX: f32 = 4.0;

pub const FLUX_ATTACK_MIN: f32 = 0.001;
pub const FLUX_ATTACK_MAX: f32 = 0.1;
pub const FLUX_RELEASE_MIN: f32 = 0.005;
pub const FLUX_RELEASE_MAX: f32 = 0.5;
pub const FLUX_AVERAGE_MIN: f32 = 0.05;
pub const FLUX_AVERAGE_MAX: f32 = 4.0;
/// Largest number of onset peaks the adaptive reference reserves room for.
/// The time window bounds the count in practice.
const PEAK_WINDOW_CAPACITY: usize = 256;
pub const FLUX_SENSITIVITY_MIN: f32 = 0.5;
pub const FLUX_SENSITIVITY_MAX: f32 = 5.0;
pub const FLUX_THRESHOLD_MIN: f32 = 0.0;
/// Upper bound of the dedicated composite-onset minimum threshold.
pub const ONSET_THRESHOLD_MAX: f32 = 0.25;
pub const FLUX_INTERVAL_MIN: f32 = 0.005;
pub const FLUX_INTERVAL_MAX: f32 = 0.2;
pub const FLUX_SUPPRESSION_MIN: f32 = 0.0;
pub const FLUX_SUPPRESSION_MAX: f32 = 2.0;
pub const FLUX_PRE_GAIN_MIN: f32 = 0.01;
pub const FLUX_PRE_GAIN_MAX: f32 = 4.0;
pub const CHOKE_MIN: f32 = 0.0;
pub const CHOKE_MAX: f32 = 2.0;
pub const QUAKE_FACTOR_MIN: f32 = 0.0;
pub const QUAKE_FACTOR_MAX: f32 = 50.0;
pub const SEQUENCE_EVERY_MIN: u32 = 1;
pub const SEQUENCE_EVERY_MAX: u32 = 32;
/// Allowed snare output RMS gate in dBFS.
pub const SNARE_RMS_GATE_MIN_DB: f32 = -60.0;
pub const SNARE_RMS_GATE_MAX_DB: f32 = 0.0;
pub const SNARE_RMS_GATE_DEFAULT_DB: f32 = -45.0;
/// Allowed gap gate level in dBFS.
pub const DROP_GAP_THRESHOLD_MIN: f32 = -60.0;
pub const DROP_GAP_THRESHOLD_MAX: f32 = 0.0;
pub const DROP_KERNEL_MIN: f32 = 0.0;
pub const DROP_KERNEL_MAX: f32 = 1.0;
pub const SPRING_STIFFNESS_MIN: f32 = 1.0;
pub const SPRING_STIFFNESS_MAX: f32 = 400.0;
pub const SPRING_DAMPING_MIN: f32 = 0.0;
pub const SPRING_DAMPING_MAX: f32 = 60.0;

const FLASH_DECAY: f32 = 8.0;
const SPRING_STEP: f32 = 1.0 / 240.0;
/// RMS envelope follower time constants. The release outlives the 85 ms
/// analysis window so a short dip between beats cannot arm the gap gate.
const GAP_ENVELOPE_ATTACK: f32 = 0.02;
const GAP_ENVELOPE_RELEASE: f32 = 0.35;
/// Silence floor for every level reported in dBFS.
const GAP_DB_FLOOR: f32 = -80.0;
const KERNEL_EPSILON: f32 = 1e-6;

/// The drop detector's onset kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropKernel {
    Onset,
}

impl DropKernel {
    pub const ALL: [Self; 1] = [Self::Onset];

    pub fn label(self) -> &'static str {
        match self {
            Self::Onset => "ONSET",
        }
    }
}

/// Per-band weights for the drop detector's onset projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropKernels {
    pub onset: [f32; BAND_COUNT],
}

impl Default for DropKernels {
    fn default() -> Self {
        Self {
            onset: [
                0.44955054, 0.6159004, 0.7489803, 0.8598802, 0.89315015, 0.8709702, 0.77116024,
                0.6269904, 0.5050005, 0.4051906, 0.31647065, 0.28320068, 0.2721107, 0.28320068,
                0.3608306, 0.67135036,
            ],
        }
    }
}

impl DropKernels {
    pub fn weights(self, kernel: DropKernel) -> [f32; BAND_COUNT] {
        match kernel {
            DropKernel::Onset => self.onset,
        }
    }

    pub fn set(&mut self, kernel: DropKernel, band: usize, value: f32) {
        assert!(
            band < BAND_COUNT,
            "drop kernel bands stay inside the spectrum"
        );
        let value = value.clamp(DROP_KERNEL_MIN, DROP_KERNEL_MAX);
        match kernel {
            DropKernel::Onset => self.onset[band] = value,
        }
    }

    fn validate(self) -> anyhow::Result<()> {
        for kernel in DropKernel::ALL {
            validate_kernel(kernel.label(), &self.weights(kernel))?;
        }
        Ok(())
    }
}

fn kernel_projection(weights: &[f32; BAND_COUNT], values: &[f32; BAND_COUNT]) -> f32 {
    let total = weights.iter().sum::<f32>();
    if total <= KERNEL_EPSILON {
        return 0.0;
    }
    weights
        .iter()
        .zip(values)
        .map(|(weight, value)| weight * value)
        .sum::<f32>()
        / total
}

fn validate_kernel(name: &str, weights: &[f32; BAND_COUNT]) -> anyhow::Result<()> {
    for (band, weight) in weights.iter().enumerate() {
        anyhow::ensure!(
            weight.is_finite() && (DROP_KERNEL_MIN..=DROP_KERNEL_MAX).contains(weight),
            "{name} kernel band {band} must be finite and between \
             {DROP_KERNEL_MIN} and {DROP_KERNEL_MAX}"
        );
    }
    Ok(())
}

/// Per-band absolute-loudness shaping. The tracked high reference is scaled by
/// the gain and then passed through the curve.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BandOnsetSettings {
    pub abl_gain: f32,
    pub abl_curve: Curve,
    pub abl_amount: f32,
}

impl Default for BandOnsetSettings {
    fn default() -> Self {
        Self {
            abl_gain: 1.0,
            abl_curve: Curve::SCurve,
            abl_amount: 2.5,
        }
    }
}

impl BandOnsetSettings {
    pub fn loudness(self, high: f32) -> f32 {
        let scaled = (high * self.abl_gain).clamp(0.0, 1.0);
        self.abl_curve.apply(scaled, self.abl_amount)
    }
}

/// Largest number of analysis steps a half-second template can span.
pub const PATTERN_MAX_STEPS: usize = 192;

/// Which of the two temporal template polygons a vertex belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternLayer {
    Inner,
    Outer,
}

/// Which of the two temporal templates a point belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PatternKernel {
    #[default]
    Main,
    Suppression,
}

/// Normalized time-by-band polygon vertices. Time runs left to right and band
/// runs top to bottom, both in 0..1.
pub type PatternPolygon = Vec<[f32; 2]>;

/// A time-by-band onset template. The template is always half a second long;
/// two polygons define its field: the inner polygon is the maximum, the outer
/// polygon is the zero boundary, and the region between them is interpolated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalPattern {
    pub inner: PatternPolygon,
    pub outer: PatternPolygon,
    pub suppression_inner: PatternPolygon,
    pub suppression_outer: PatternPolygon,
    /// Fraction of the half-second template used for matching, 0.1..1.0.
    #[serde(default = "default_pattern_area")]
    pub effective: f32,
}

fn default_pattern_area() -> f32 {
    1.0
}

impl Default for TemporalPattern {
    fn default() -> Self {
        Self::kick_initial()
    }
}

impl TemporalPattern {
    pub fn kick_initial() -> Self {
        Self {
            inner: vec![
                [0.0, 0.33803162],
                [0.032534927, 0.15787412],
                [0.10334638, 0.04483413],
                [0.16458872, 0.0],
                [0.3042978, 0.0],
                [0.26984897, 0.10135413],
                [0.16267489, 0.16140662],
                [0.07655285, 0.34862912],
                [0.02296581, 0.5782416],
                [0.0038275793, 0.7619316],
                [0.034291036, 0.878736],
                [0.038276393, 1.0],
                [0.0, 1.0],
            ],
            outer: vec![
                [0.0, 0.23205662],
                [0.028707279, 0.08369163],
                [0.12248461, 0.0],
                [0.33491898, 0.0],
                [0.27559045, 0.16140662],
                [0.16458872, 0.19673163],
                [0.09760491, 0.42987663],
                [0.049759332, 0.5853066],
                [0.049759332, 0.7407366],
                [0.06124227, 1.0],
                [0.0, 1.0],
            ],
            suppression_inner: vec![
                [0.0, 0.0],
                [0.075829335, 0.0],
                [0.019845905, 0.08775653],
                [0.0, 0.26498532],
            ],
            suppression_outer: vec![
                [0.0, 0.0],
                [0.1090047, 0.0],
                [0.040580507, 0.102450065],
                [0.0021032926, 0.31054506],
            ],
            effective: 0.22,
        }
    }

    pub fn snare_initial() -> Self {
        Self {
            inner: vec![
                [0.019887151, 0.28172547],
                [0.144943, 0.29345784],
                [0.15471299, 0.43893924],
                [0.007405143, 0.48870692],
                [0.071682416, 0.5504716],
                [0.113679044, 0.7862174],
                [0.064829096, 0.8730369],
                [0.011552064, 0.9736742],
                [0.0, 0.98968726],
                [0.0, 0.43838012],
            ],
            outer: vec![
                [0.0, 0.4132167],
                [0.0003471733, 0.24652837],
                [0.15862098, 0.24652837],
                [0.2270109, 0.43893924],
                [0.17328197, 0.5024324],
                [0.16706158, 0.7609291],
                [0.0687371, 0.93169874],
                [0.019845905, 1.0],
                [0.0, 1.0],
                [0.0, 0.89132124],
            ],
            suppression_inner: vec![
                [0.0, 0.0],
                [0.08619663, 0.0],
                [0.09023107, 0.20898479],
                [0.008163164, 0.21367773],
            ],
            suppression_outer: vec![
                [0.0, 0.0],
                [0.16706158, 0.0],
                [0.13321902, 0.27468607],
                [0.0, 0.29111138],
            ],
            effective: 0.14,
        }
    }

    /// A short, high-band transient that follows a hi-hat rather than a snare.
    pub fn hat_initial() -> Self {
        Self {
            inner: vec![
                [0.0, 0.62],
                [0.055, 0.58],
                [0.11, 0.64],
                [0.15, 0.78],
                [0.12, 0.90],
                [0.05, 1.0],
                [0.0, 1.0],
            ],
            outer: vec![
                [0.0, 0.50],
                [0.09, 0.46],
                [0.19, 0.56],
                [0.23, 0.76],
                [0.20, 0.94],
                [0.09, 1.0],
                [0.0, 1.0],
            ],
            suppression_inner: vec![[0.0, 0.0], [0.07, 0.0], [0.09, 0.22], [0.0, 0.26]],
            suppression_outer: vec![[0.0, 0.0], [0.12, 0.0], [0.15, 0.30], [0.0, 0.36]],
            effective: 0.18,
        }
    }

    /// Approximates a previous step-grid pattern with a band-region rectangle.
    pub fn from_legacy_steps(steps: &[[f32; BAND_COUNT]]) -> Self {
        let mut minimum = BAND_COUNT;
        let mut maximum = 0;
        for step in steps {
            for (band, value) in step.iter().enumerate() {
                if *value > 0.0 {
                    minimum = minimum.min(band);
                    maximum = maximum.max(band);
                }
            }
        }
        if minimum >= BAND_COUNT {
            return Self::kick_initial();
        }
        let low = minimum as f32 / BAND_COUNT as f32;
        let high = (maximum + 1) as f32 / BAND_COUNT as f32;
        let start = if steps.len() == 1 { 0.8 } else { 0.0 };
        Self {
            inner: rectangle(start, 1.0, low, high),
            outer: rectangle(start, 1.0, (low - 0.06).max(0.0), (high + 0.06).min(1.0)),
            suppression_inner: Vec::new(),
            suppression_outer: Vec::new(),
            effective: 1.0,
        }
    }

    pub fn contains_band(&self, band: usize) -> bool {
        let y = (band as f32 + 0.5) / BAND_COUNT as f32;
        (0..16).any(|step| self.field_value((step as f32 + 0.5) / 16.0, y) > 0.01)
    }

    fn validate(&self) -> anyhow::Result<()> {
        validate_polygon("inner", &self.inner)?;
        validate_polygon("outer", &self.outer)?;
        validate_polygon("suppression inner", &self.suppression_inner)?;
        validate_polygon("suppression outer", &self.suppression_outer)?;
        anyhow::ensure!(
            self.effective.is_finite() && (0.1..=1.0).contains(&self.effective),
            "temporal pattern effective area must be between 10% and 100%"
        );
        Ok(())
    }

    pub fn polygons(&self, kernel: PatternKernel) -> (&PatternPolygon, &PatternPolygon) {
        match kernel {
            PatternKernel::Main => (&self.inner, &self.outer),
            PatternKernel::Suppression => (&self.suppression_inner, &self.suppression_outer),
        }
    }

    /// Field value at normalized time and band.
    pub fn field_value(&self, x: f32, y: f32) -> f32 {
        field_value_for(&self.inner, &self.outer, x, y)
    }

    pub fn suppression_field_value(&self, x: f32, y: f32) -> f32 {
        field_value_for(&self.suppression_inner, &self.suppression_outer, x, y)
    }

    /// Effective field at normalized time and band. The retained part starts
    /// at the template start; the end of the effective area is faded over its
    /// last 10%.
    pub fn effective_field_value(&self, x: f32, y: f32) -> f32 {
        self.effective_field_value_for(PatternKernel::Main, x, y)
    }

    pub fn suppression_effective_field_value(&self, x: f32, y: f32) -> f32 {
        self.effective_field_value_for(PatternKernel::Suppression, x, y)
    }

    pub fn effective_field_value_for(&self, kernel: PatternKernel, x: f32, y: f32) -> f32 {
        let effective = self.effective.clamp(0.1, 1.0);
        if x > effective {
            return 0.0;
        }
        let within = (x / effective).clamp(0.0, 1.0);
        let fade = if effective < 1.0 {
            ((1.0 - within) / 0.1).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let (inner, outer) = self.polygons(kernel);
        field_value_for(inner, outer, x, y) * fade
    }

    fn sample_steps(&self, kernel: PatternKernel, steps: usize) -> Vec<[f32; BAND_COUNT]> {
        let effective = self.effective.clamp(0.1, 1.0);
        (0..steps)
            .map(|step| {
                let within = (step as f32 + 0.5) / steps as f32;
                let x = within * effective;
                std::array::from_fn(|band| {
                    self.effective_field_value_for(
                        kernel,
                        x,
                        (band as f32 + 0.5) / BAND_COUNT as f32,
                    )
                })
            })
            .collect()
    }

    /// Matches the template against the newest part of the onset history and
    /// returns the main-kernel excitation, the suppression-kernel inhibition,
    /// and their residual after suppression.
    fn project(
        &self,
        history: &VecDeque<[f32; BAND_COUNT]>,
        suppression: f32,
        pre_gain: f32,
        steps: usize,
    ) -> OnsetProjection {
        let take = steps.clamp(1, PATTERN_MAX_STEPS).min(history.len());
        if take == 0 {
            return OnsetProjection::default();
        }
        let effective_steps =
            ((take as f32 * self.effective.clamp(0.1, 1.0)).ceil() as usize).clamp(1, take);
        let offset = history.len() - effective_steps;
        let pattern = self.sample_steps(PatternKernel::Main, effective_steps);
        let suppressor_weights = self.sample_steps(PatternKernel::Suppression, effective_steps);
        let mut excited = 0.0;
        let mut excitation_weight = 0.0;
        let mut inhibited = 0.0;
        let mut inhibition_weight = 0.0;
        for (step_index, weights) in pattern.iter().enumerate() {
            let flow = &history[offset + step_index];
            for band in 0..BAND_COUNT {
                let weight = weights[band].clamp(0.0, 1.0);
                excited += weight * flow[band];
                excitation_weight += weight;
                let suppressor = suppressor_weights[step_index][band].clamp(0.0, 1.0);
                inhibited += suppressor * flow[band];
                inhibition_weight += suppressor;
            }
        }
        let excitation = if excitation_weight > KERNEL_EPSILON {
            excited / excitation_weight
        } else {
            0.0
        } * pre_gain;
        let inhibition = if inhibition_weight > KERNEL_EPSILON {
            inhibited / inhibition_weight
        } else {
            0.0
        } * pre_gain;
        let suppressed = suppression * inhibition;
        OnsetProjection {
            inhibition,
            suppressed,
            residual: (excitation - suppressed).max(0.0),
        }
    }
}

fn rectangle(x0: f32, x1: f32, y0: f32, y1: f32) -> PatternPolygon {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn field_value_for(inner: &[[f32; 2]], outer: &[[f32; 2]], x: f32, y: f32) -> f32 {
    if inner.len() < 3 || outer.len() < 3 {
        return 0.0;
    }
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return 0.0;
    }
    if point_in_polygon(inner, x, y) {
        return 1.0;
    }
    if !point_in_polygon(outer, x, y) {
        return 0.0;
    }
    let inner_distance = distance_to_polygon(inner, x, y).max(KERNEL_EPSILON);
    let outer_distance = distance_to_polygon(outer, x, y).max(KERNEL_EPSILON);
    (outer_distance / (inner_distance + outer_distance)).clamp(0.0, 1.0)
}

fn validate_polygon(name: &str, points: &[[f32; 2]]) -> anyhow::Result<()> {
    anyhow::ensure!(
        (3..=32).contains(&points.len()),
        "temporal {name} polygon must have between 3 and 32 vertices"
    );
    for (index, point) in points.iter().enumerate() {
        anyhow::ensure!(
            point[0].is_finite()
                && point[1].is_finite()
                && (0.0..=1.0).contains(&point[0])
                && (0.0..=1.0).contains(&point[1]),
            "temporal {name} vertex {index} must be inside the normalized grid"
        );
    }
    Ok(())
}

fn point_in_polygon(points: &[[f32; 2]], x: f32, y: f32) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = points.len() - 1;
    for current in 0..points.len() {
        let a = points[current];
        let b = points[previous];
        let crosses = (a[1] > y) != (b[1] > y)
            && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1] + f32::EPSILON) + a[0];
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn distance_to_polygon(points: &[[f32; 2]], x: f32, y: f32) -> f32 {
    let mut distance = f32::INFINITY;
    if points.len() < 2 {
        return distance;
    }
    let mut previous = points.len() - 1;
    for current in 0..points.len() {
        distance = distance.min(distance_to_segment(points[previous], points[current], x, y));
        previous = current;
    }
    distance
}

fn distance_to_segment(a: [f32; 2], b: [f32; 2], x: f32, y: f32) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let length_squared = dx * dx + dy * dy;
    if length_squared <= f32::EPSILON {
        return ((x - a[0]).powi(2) + (y - a[1]).powi(2)).sqrt();
    }
    let t = (((x - a[0]) * dx + (y - a[1]) * dy) / length_squared).clamp(0.0, 1.0);
    let px = a[0] + t * dx;
    let py = a[1] + t * dy;
    ((x - px).powi(2) + (y - py).powi(2)).sqrt()
}

/// All user-tunable detector configuration. The whole value persists in the
/// config file and is mirrored by the inspector sliders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "DetectorSettingsRaw", into = "DetectorSettingsRaw")]
pub struct DetectorSettings {
    /// Per-band absolute-loudness shaping used by the onset flow.
    pub band_onset: [BandOnsetSettings; BAND_COUNT],
    /// Per-bin relative-excess background detector.
    pub excess: RelativeExcessSettings,
    /// Temporal onset template for the kick detector.
    pub kick_pattern: TemporalPattern,
    /// Trim applied to the kick pattern projection before onset tracking.
    pub kick_onset_pre_gain: f32,
    /// Dedicated onset tracker settings for the kick onset kernel.
    pub kick_onset_attack: f32,
    pub kick_onset_release: f32,
    pub kick_onset_average: f32,
    pub kick_onset_sensitivity: f32,
    pub kick_onset_min_threshold: f32,
    pub kick_onset_min_interval: f32,
    /// Weight of the kick suppression-kernel flow subtracted from its onset flow.
    pub kick_onset_suppression: f32,
    /// Time the kick stays muted after a confirmed snare or drop.
    pub kick_snare_choke: f32,
    pub kick_drop_choke: f32,
    pub kick_quake_factor: f32,
    /// A new quake direction is taken every this many kicks.
    pub kick_sequence_every: u32,
    /// Temporal onset template for the snare detector.
    pub snare_pattern: TemporalPattern,
    /// Trim applied to the snare pattern projection before onset tracking.
    pub snare_onset_pre_gain: f32,
    /// Dedicated onset tracker settings for the snare onset kernel.
    pub snare_onset_attack: f32,
    pub snare_onset_release: f32,
    pub snare_onset_average: f32,
    pub snare_onset_sensitivity: f32,
    pub snare_onset_min_threshold: f32,
    pub snare_onset_min_interval: f32,
    /// Weight of the snare suppression-kernel flow subtracted from its onset flow.
    pub snare_onset_suppression: f32,
    /// Minimum broadband RMS in dBFS required to emit a confirmed snare.
    pub snare_min_rms_db: f32,
    pub snare_flip_every: u32,
    pub snare_poi_switches: u32,
    pub snare_choke: f32,
    pub flip_stiffness: f32,
    pub flip_damping: f32,
    /// Temporal onset template for the hi-hat detector.
    pub hat_pattern: TemporalPattern,
    /// Trim applied to the hi-hat pattern projection before onset tracking.
    pub hat_onset_pre_gain: f32,
    pub hat_onset_attack: f32,
    pub hat_onset_release: f32,
    pub hat_onset_average: f32,
    pub hat_onset_sensitivity: f32,
    pub hat_onset_min_threshold: f32,
    pub hat_onset_min_interval: f32,
    /// Weight of the hi-hat suppression-kernel flow subtracted from its onset flow.
    pub hat_onset_suppression: f32,
    /// Per-band weights for the onset projection.
    pub drop_kernels: DropKernels,
    /// RMS envelope level in dBFS below which the gap gate arms.
    pub drop_gap_threshold: f32,
    /// Dedicated onset tracker settings for the composite onset kernel.
    pub drop_onset_pre_gain: f32,
    pub drop_onset_attack: f32,
    pub drop_onset_release: f32,
    pub drop_onset_average: f32,
    pub drop_onset_sensitivity: f32,
    pub drop_onset_min_threshold: f32,
    pub drop_onset_min_interval: f32,
    /// Weight of the inverted-kernel flow subtracted from the drop onset flow.
    pub drop_onset_suppression: f32,
    pub drop_quake_factor: f32,
    pub quake_stiffness: f32,
    pub quake_damping: f32,
}

impl Default for DetectorSettings {
    fn default() -> Self {
        Self {
            band_onset: [BandOnsetSettings::default(); BAND_COUNT],
            excess: RelativeExcessSettings::default(),
            kick_pattern: TemporalPattern::kick_initial(),
            kick_onset_pre_gain: 0.49823883,
            kick_onset_attack: 0.001,
            kick_onset_release: 0.010084573,
            kick_onset_average: 2.0,
            kick_onset_sensitivity: 0.5,
            kick_onset_min_threshold: 0.01,
            kick_onset_min_interval: 0.09514833,
            kick_onset_suppression: 0.5,
            kick_snare_choke: 0.099999994,
            kick_drop_choke: 0.099999994,
            kick_quake_factor: 40.0,
            kick_sequence_every: 1,
            snare_pattern: TemporalPattern::snare_initial(),
            snare_onset_pre_gain: 1.0054638,
            snare_onset_attack: 0.001,
            snare_onset_release: 0.005,
            snare_onset_average: 2.0,
            snare_onset_sensitivity: 0.5,
            snare_onset_min_threshold: 0.01,
            snare_onset_min_interval: 0.100148335,
            snare_onset_suppression: 0.32999998,
            snare_min_rms_db: SNARE_RMS_GATE_DEFAULT_DB,
            snare_flip_every: 1,
            snare_poi_switches: 3,
            snare_choke: 0.099999994,
            flip_stiffness: 349.5,
            flip_damping: 18.5,
            hat_pattern: TemporalPattern::hat_initial(),
            hat_onset_pre_gain: 1.0054638,
            hat_onset_attack: 0.001,
            hat_onset_release: 0.005,
            hat_onset_average: 1.5,
            hat_onset_sensitivity: 0.5,
            hat_onset_min_threshold: 0.01,
            hat_onset_min_interval: 0.06,
            hat_onset_suppression: 0.32999998,
            drop_kernels: DropKernels::default(),
            drop_gap_threshold: -18.0,
            drop_onset_pre_gain: 1.0054638,
            drop_onset_attack: 0.024582442,
            drop_onset_release: 0.05,
            drop_onset_average: 0.2,
            drop_onset_sensitivity: 1.5,
            drop_onset_min_threshold: 0.125,
            drop_onset_min_interval: 0.14,
            drop_onset_suppression: 0.5,
            drop_quake_factor: 20.0,
            quake_stiffness: 200.0,
            quake_damping: 2.5,
        }
    }
}

impl DetectorSettings {
    pub fn validate(self) -> anyhow::Result<Self> {
        for (band, settings) in self.band_onset.iter().enumerate() {
            anyhow::ensure!(
                settings.abl_gain.is_finite()
                    && (ABL_GAIN_MIN..=ABL_GAIN_MAX).contains(&settings.abl_gain),
                "band {band} ABL gain must be between {ABL_GAIN_MIN} and {ABL_GAIN_MAX}"
            );
            anyhow::ensure!(
                settings.abl_amount.is_finite()
                    && (ABL_AMOUNT_MIN..=Curve::MAX_AMOUNT).contains(&settings.abl_amount),
                "band {band} ABL amount must be between {ABL_AMOUNT_MIN} and {}",
                Curve::MAX_AMOUNT
            );
        }
        for (name, value, minimum, maximum) in [
            (
                "excess background_attack",
                self.excess.background_attack,
                EXCESS_ATTACK_MIN,
                EXCESS_ATTACK_MAX,
            ),
            (
                "excess background_release",
                self.excess.background_release,
                EXCESS_RELEASE_MIN,
                EXCESS_RELEASE_MAX,
            ),
            (
                "excess floor",
                self.excess.floor,
                EXCESS_FLOOR_MIN,
                EXCESS_FLOOR_MAX,
            ),
            (
                "excess cap",
                self.excess.cap,
                EXCESS_CAP_MIN,
                EXCESS_CAP_MAX,
            ),
        ] {
            anyhow::ensure!(
                value.is_finite() && (minimum..=maximum).contains(&value),
                "{name} must be finite and between {minimum} and {maximum}"
            );
        }
        for (name, value, minimum, maximum) in [
            (
                "drop_onset_pre_gain",
                self.drop_onset_pre_gain,
                FLUX_PRE_GAIN_MIN,
                FLUX_PRE_GAIN_MAX,
            ),
            (
                "drop_onset_attack",
                self.drop_onset_attack,
                FLUX_ATTACK_MIN,
                FLUX_ATTACK_MAX,
            ),
            (
                "drop_onset_release",
                self.drop_onset_release,
                FLUX_RELEASE_MIN,
                FLUX_RELEASE_MAX,
            ),
            (
                "drop_onset_average",
                self.drop_onset_average,
                FLUX_AVERAGE_MIN,
                FLUX_AVERAGE_MAX,
            ),
            (
                "drop_onset_sensitivity",
                self.drop_onset_sensitivity,
                FLUX_SENSITIVITY_MIN,
                FLUX_SENSITIVITY_MAX,
            ),
            (
                "drop_onset_min_threshold",
                self.drop_onset_min_threshold,
                FLUX_THRESHOLD_MIN,
                ONSET_THRESHOLD_MAX,
            ),
            (
                "drop_onset_min_interval",
                self.drop_onset_min_interval,
                FLUX_INTERVAL_MIN,
                FLUX_INTERVAL_MAX,
            ),
            (
                "drop_onset_suppression",
                self.drop_onset_suppression,
                FLUX_SUPPRESSION_MIN,
                FLUX_SUPPRESSION_MAX,
            ),
        ] {
            anyhow::ensure!(
                value.is_finite() && (minimum..=maximum).contains(&value),
                "{name} must be finite and between {minimum} and {maximum}"
            );
        }
        for (name, value, minimum, maximum) in [
            (
                "kick_onset_pre_gain",
                self.kick_onset_pre_gain,
                FLUX_PRE_GAIN_MIN,
                FLUX_PRE_GAIN_MAX,
            ),
            (
                "kick_onset_attack",
                self.kick_onset_attack,
                FLUX_ATTACK_MIN,
                FLUX_ATTACK_MAX,
            ),
            (
                "kick_onset_release",
                self.kick_onset_release,
                FLUX_RELEASE_MIN,
                FLUX_RELEASE_MAX,
            ),
            (
                "kick_onset_average",
                self.kick_onset_average,
                FLUX_AVERAGE_MIN,
                FLUX_AVERAGE_MAX,
            ),
            (
                "kick_onset_sensitivity",
                self.kick_onset_sensitivity,
                FLUX_SENSITIVITY_MIN,
                FLUX_SENSITIVITY_MAX,
            ),
            (
                "kick_onset_min_threshold",
                self.kick_onset_min_threshold,
                FLUX_THRESHOLD_MIN,
                ONSET_THRESHOLD_MAX,
            ),
            (
                "kick_onset_min_interval",
                self.kick_onset_min_interval,
                FLUX_INTERVAL_MIN,
                FLUX_INTERVAL_MAX,
            ),
            (
                "kick_onset_suppression",
                self.kick_onset_suppression,
                FLUX_SUPPRESSION_MIN,
                FLUX_SUPPRESSION_MAX,
            ),
            (
                "snare_onset_pre_gain",
                self.snare_onset_pre_gain,
                FLUX_PRE_GAIN_MIN,
                FLUX_PRE_GAIN_MAX,
            ),
            (
                "snare_onset_attack",
                self.snare_onset_attack,
                FLUX_ATTACK_MIN,
                FLUX_ATTACK_MAX,
            ),
            (
                "snare_onset_release",
                self.snare_onset_release,
                FLUX_RELEASE_MIN,
                FLUX_RELEASE_MAX,
            ),
            (
                "snare_onset_average",
                self.snare_onset_average,
                FLUX_AVERAGE_MIN,
                FLUX_AVERAGE_MAX,
            ),
            (
                "snare_onset_sensitivity",
                self.snare_onset_sensitivity,
                FLUX_SENSITIVITY_MIN,
                FLUX_SENSITIVITY_MAX,
            ),
            (
                "snare_onset_min_threshold",
                self.snare_onset_min_threshold,
                FLUX_THRESHOLD_MIN,
                ONSET_THRESHOLD_MAX,
            ),
            (
                "snare_onset_min_interval",
                self.snare_onset_min_interval,
                FLUX_INTERVAL_MIN,
                FLUX_INTERVAL_MAX,
            ),
            (
                "snare_onset_suppression",
                self.snare_onset_suppression,
                FLUX_SUPPRESSION_MIN,
                FLUX_SUPPRESSION_MAX,
            ),
            (
                "snare_min_rms_db",
                self.snare_min_rms_db,
                SNARE_RMS_GATE_MIN_DB,
                SNARE_RMS_GATE_MAX_DB,
            ),
            (
                "hat_onset_pre_gain",
                self.hat_onset_pre_gain,
                FLUX_PRE_GAIN_MIN,
                FLUX_PRE_GAIN_MAX,
            ),
            (
                "hat_onset_attack",
                self.hat_onset_attack,
                FLUX_ATTACK_MIN,
                FLUX_ATTACK_MAX,
            ),
            (
                "hat_onset_release",
                self.hat_onset_release,
                FLUX_RELEASE_MIN,
                FLUX_RELEASE_MAX,
            ),
            (
                "hat_onset_average",
                self.hat_onset_average,
                FLUX_AVERAGE_MIN,
                FLUX_AVERAGE_MAX,
            ),
            (
                "hat_onset_sensitivity",
                self.hat_onset_sensitivity,
                FLUX_SENSITIVITY_MIN,
                FLUX_SENSITIVITY_MAX,
            ),
            (
                "hat_onset_min_threshold",
                self.hat_onset_min_threshold,
                FLUX_THRESHOLD_MIN,
                ONSET_THRESHOLD_MAX,
            ),
            (
                "hat_onset_min_interval",
                self.hat_onset_min_interval,
                FLUX_INTERVAL_MIN,
                FLUX_INTERVAL_MAX,
            ),
            (
                "hat_onset_suppression",
                self.hat_onset_suppression,
                FLUX_SUPPRESSION_MIN,
                FLUX_SUPPRESSION_MAX,
            ),
        ] {
            anyhow::ensure!(
                value.is_finite() && (minimum..=maximum).contains(&value),
                "{name} must be finite and between {minimum} and {maximum}"
            );
        }
        self.kick_pattern.validate()?;
        self.snare_pattern.validate()?;
        self.hat_pattern.validate()?;
        for (name, choke) in [
            ("kick_snare_choke", self.kick_snare_choke),
            ("kick_drop_choke", self.kick_drop_choke),
            ("snare_choke", self.snare_choke),
        ] {
            anyhow::ensure!(
                choke.is_finite() && (CHOKE_MIN..=CHOKE_MAX).contains(&choke),
                "{name} must be finite and between {CHOKE_MIN} and {CHOKE_MAX}"
            );
        }
        for (name, factor) in [
            ("kick_quake_factor", self.kick_quake_factor),
            ("drop_quake_factor", self.drop_quake_factor),
        ] {
            anyhow::ensure!(
                factor.is_finite() && (QUAKE_FACTOR_MIN..=QUAKE_FACTOR_MAX).contains(&factor),
                "{name} must be finite and between {QUAKE_FACTOR_MIN} and {QUAKE_FACTOR_MAX}"
            );
        }
        anyhow::ensure!(
            (SEQUENCE_EVERY_MIN..=SEQUENCE_EVERY_MAX).contains(&self.kick_sequence_every),
            "the kick quake sequence length must be between {SEQUENCE_EVERY_MIN} and {SEQUENCE_EVERY_MAX}"
        );
        anyhow::ensure!(
            (SEQUENCE_EVERY_MIN..=SEQUENCE_EVERY_MAX).contains(&self.snare_flip_every),
            "the snare flip count must be between {SEQUENCE_EVERY_MIN} and {SEQUENCE_EVERY_MAX}"
        );
        anyhow::ensure!(
            (SEQUENCE_EVERY_MIN..=SEQUENCE_EVERY_MAX).contains(&self.snare_poi_switches),
            "the POI switch count must be between {SEQUENCE_EVERY_MIN} and {SEQUENCE_EVERY_MAX}"
        );
        self.drop_kernels.validate()?;
        anyhow::ensure!(
            self.drop_gap_threshold.is_finite()
                && (DROP_GAP_THRESHOLD_MIN..=DROP_GAP_THRESHOLD_MAX)
                    .contains(&self.drop_gap_threshold),
            "the drop gap threshold must be finite and between \
             {DROP_GAP_THRESHOLD_MIN} and {DROP_GAP_THRESHOLD_MAX} dBFS"
        );
        for (name, stiffness) in [
            ("flip_stiffness", self.flip_stiffness),
            ("quake_stiffness", self.quake_stiffness),
        ] {
            anyhow::ensure!(
                stiffness.is_finite()
                    && (SPRING_STIFFNESS_MIN..=SPRING_STIFFNESS_MAX).contains(&stiffness),
                "{name} must be finite and between {SPRING_STIFFNESS_MIN} and {SPRING_STIFFNESS_MAX}"
            );
        }
        for (name, damping) in [
            ("flip_damping", self.flip_damping),
            ("quake_damping", self.quake_damping),
        ] {
            anyhow::ensure!(
                damping.is_finite() && (SPRING_DAMPING_MIN..=SPRING_DAMPING_MAX).contains(&damping),
                "{name} must be finite and between {SPRING_DAMPING_MIN} and {SPRING_DAMPING_MAX}"
            );
        }
        Ok(self)
    }

    fn drop_onset_recipe(&self) -> OnsetRecipe {
        OnsetRecipe {
            kernel: self.drop_kernels.onset,
            suppression: self.drop_onset_suppression,
            pre_gain: self.drop_onset_pre_gain,
            tracker: FluxSettings {
                attack: self.drop_onset_attack,
                release: self.drop_onset_release,
                average: self.drop_onset_average,
                sensitivity: self.drop_onset_sensitivity,
                min_threshold: self.drop_onset_min_threshold,
                min_interval: self.drop_onset_min_interval,
            },
        }
    }

    fn kick_onset_tracker(&self) -> FluxSettings {
        FluxSettings {
            attack: self.kick_onset_attack,
            release: self.kick_onset_release,
            average: self.kick_onset_average,
            sensitivity: self.kick_onset_sensitivity,
            min_threshold: self.kick_onset_min_threshold,
            min_interval: self.kick_onset_min_interval,
        }
    }

    fn snare_onset_tracker(&self) -> FluxSettings {
        FluxSettings {
            attack: self.snare_onset_attack,
            release: self.snare_onset_release,
            average: self.snare_onset_average,
            sensitivity: self.snare_onset_sensitivity,
            min_threshold: self.snare_onset_min_threshold,
            min_interval: self.snare_onset_min_interval,
        }
    }

    fn hat_onset_tracker(&self) -> FluxSettings {
        FluxSettings {
            attack: self.hat_onset_attack,
            release: self.hat_onset_release,
            average: self.hat_onset_average,
            sensitivity: self.hat_onset_sensitivity,
            min_threshold: self.hat_onset_min_threshold,
            min_interval: self.hat_onset_min_interval,
        }
    }

    pub fn kick_contains(&self, band: usize) -> bool {
        self.kick_pattern.contains_band(band)
    }

    pub fn snare_contains(&self, band: usize) -> bool {
        self.snare_pattern.contains_band(band)
    }

    pub fn hat_contains(&self, band: usize) -> bool {
        self.hat_pattern.contains_band(band)
    }

    pub fn drop_kernel_weights(&self, kernel: DropKernel) -> [f32; BAND_COUNT] {
        self.drop_kernels.weights(kernel)
    }

    pub fn set_drop_kernel_band(&mut self, kernel: DropKernel, band: usize, value: f32) {
        assert!(band < BAND_COUNT, "kernel bands stay inside the spectrum");
        self.drop_kernels.set(kernel, band, value);
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BandFluxSettingsRaw {
    attack: f32,
    release: f32,
    average: f32,
    sensitivity: f32,
    min_threshold: f32,
    min_interval: f32,
    #[serde(default = "default_flux_gain")]
    gain: f32,
}

fn default_flux_gain() -> f32 {
    1.0
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OptionalKernel {
    Bare([f32; BAND_COUNT]),
    Wrapped(Option<[f32; BAND_COUNT]>),
}

fn deserialize_optional_kernel<'de, D>(
    deserializer: D,
) -> Result<Option<[f32; BAND_COUNT]>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match OptionalKernel::deserialize(deserializer)? {
        OptionalKernel::Bare(kernel) => Some(kernel),
        OptionalKernel::Wrapped(kernel) => kernel,
    })
}

fn deserialize_optional_f32<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    f32::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TemporalPatternRaw {
    inner: PatternPolygon,
    outer: PatternPolygon,
    #[serde(default)]
    suppression_inner: PatternPolygon,
    #[serde(default)]
    suppression_outer: PatternPolygon,
    #[serde(skip_serializing)]
    steps: Vec<[f32; BAND_COUNT]>,
    #[serde(default = "default_pattern_area")]
    effective: f32,
}

impl Default for TemporalPatternRaw {
    fn default() -> Self {
        Self {
            inner: Vec::new(),
            outer: Vec::new(),
            suppression_inner: Vec::new(),
            suppression_outer: Vec::new(),
            steps: Vec::new(),
            effective: 1.0,
        }
    }
}

impl TemporalPatternRaw {
    fn into_pattern(self) -> TemporalPattern {
        if self.inner.len() >= 3 && self.outer.len() >= 3 {
            TemporalPattern {
                inner: self.inner,
                outer: self.outer,
                suppression_inner: self.suppression_inner,
                suppression_outer: self.suppression_outer,
                effective: self.effective,
            }
        } else if !self.steps.is_empty() {
            TemporalPattern::from_legacy_steps(&self.steps)
        } else {
            TemporalPattern::kick_initial()
        }
    }
}

impl From<TemporalPattern> for TemporalPatternRaw {
    fn from(pattern: TemporalPattern) -> Self {
        Self {
            inner: pattern.inner,
            outer: pattern.outer,
            suppression_inner: pattern.suppression_inner,
            suppression_outer: pattern.suppression_outer,
            steps: Vec::new(),
            effective: pattern.effective,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DetectorSettingsRaw {
    /// New per-band ABL settings. Missing means all bands use defaults.
    band_onset: Vec<BandOnsetSettings>,
    excess: RelativeExcessSettings,
    /// Settings from the previous per-band flux tracker; accepted but unused.
    #[serde(rename = "band_flux", skip_serializing)]
    legacy_band_flux: Vec<BandFluxSettingsRaw>,
    #[serde(rename = "flux_attack", skip_serializing)]
    legacy_flux_attack: f32,
    #[serde(rename = "flux_release", skip_serializing)]
    legacy_flux_release: f32,
    #[serde(rename = "flux_average", skip_serializing)]
    legacy_flux_average: f32,
    #[serde(rename = "flux_sensitivity", skip_serializing)]
    legacy_flux_sensitivity: f32,
    #[serde(rename = "flux_min_threshold", skip_serializing)]
    legacy_flux_min_threshold: f32,
    #[serde(rename = "flux_min_interval", skip_serializing)]
    legacy_flux_min_interval: f32,
    kick_pattern: Option<TemporalPatternRaw>,
    #[serde(
        rename = "kick_kernel",
        skip_serializing,
        default,
        deserialize_with = "deserialize_optional_kernel"
    )]
    legacy_kick_kernel: Option<[f32; BAND_COUNT]>,
    kick_onset_pre_gain: f32,
    kick_onset_attack: f32,
    kick_onset_release: f32,
    kick_onset_average: f32,
    kick_onset_sensitivity: f32,
    kick_onset_min_threshold: f32,
    kick_onset_min_interval: f32,
    kick_onset_suppression: f32,
    kick_snare_choke: f32,
    kick_drop_choke: f32,
    kick_quake_factor: f32,
    kick_sequence_every: u32,
    snare_pattern: Option<TemporalPatternRaw>,
    #[serde(
        rename = "snare_kernel",
        skip_serializing,
        default,
        deserialize_with = "deserialize_optional_kernel"
    )]
    legacy_snare_kernel: Option<[f32; BAND_COUNT]>,
    snare_onset_pre_gain: f32,
    snare_onset_attack: f32,
    snare_onset_release: f32,
    snare_onset_average: f32,
    snare_onset_sensitivity: f32,
    snare_onset_min_threshold: f32,
    snare_onset_min_interval: f32,
    snare_onset_suppression: f32,
    snare_min_rms_db: f32,
    snare_flip_every: u32,
    snare_poi_switches: u32,
    snare_choke: f32,
    flip_stiffness: f32,
    flip_damping: f32,
    hat_pattern: Option<TemporalPatternRaw>,
    hat_onset_pre_gain: f32,
    hat_onset_attack: f32,
    hat_onset_release: f32,
    hat_onset_average: f32,
    hat_onset_sensitivity: f32,
    hat_onset_min_threshold: f32,
    hat_onset_min_interval: f32,
    hat_onset_suppression: f32,
    drop_gap_threshold: f32,
    drop_onset_pre_gain: f32,
    drop_onset_attack: f32,
    drop_onset_release: f32,
    drop_onset_average: f32,
    drop_onset_sensitivity: f32,
    drop_onset_min_threshold: f32,
    drop_onset_min_interval: f32,
    drop_onset_suppression: f32,
    drop_onset_kernel: [f32; BAND_COUNT],
    drop_quake_factor: f32,
    quake_stiffness: f32,
    quake_damping: f32,
    #[serde(rename = "drop_split", skip_serializing)]
    legacy_drop_split: usize,
    #[serde(rename = "drop_onsets", skip_serializing)]
    legacy_drop_onsets: usize,
    #[serde(rename = "drop_window", skip_serializing)]
    legacy_drop_window: f32,
    #[serde(
        rename = "drop_gap_window",
        skip_serializing,
        default,
        deserialize_with = "deserialize_optional_f32"
    )]
    legacy_drop_gap_window: Option<f32>,
    #[serde(rename = "drop_gap_gain", skip_serializing)]
    legacy_drop_gap_gain: f32,
    #[serde(rename = "drop_low_lp", skip_serializing)]
    legacy_drop_low_lp: f32,
    #[serde(rename = "drop_high_lp", skip_serializing)]
    legacy_drop_high_lp: f32,
    #[serde(rename = "drop_low_kernel", skip_serializing)]
    legacy_drop_low_kernel: [f32; BAND_COUNT],
    #[serde(rename = "drop_high_kernel", skip_serializing)]
    legacy_drop_high_kernel: [f32; BAND_COUNT],
    #[serde(rename = "drop_gap_level", skip_serializing)]
    legacy_drop_gap_level: f32,
    #[serde(rename = "drop_gap_time", skip_serializing)]
    legacy_drop_gap_time: f32,
    #[serde(rename = "drop_quiet_level", skip_serializing)]
    legacy_drop_quiet_level: f32,
    #[serde(rename = "drop_quiet_time", skip_serializing)]
    legacy_drop_quiet_time: f32,
    #[serde(rename = "kick_low", skip_serializing)]
    legacy_kick_low: usize,
    #[serde(rename = "kick_high", skip_serializing)]
    legacy_kick_high: usize,
    #[serde(rename = "kick_window", skip_serializing)]
    legacy_kick_window: f32,
    #[serde(rename = "snare_bands", skip_serializing)]
    legacy_snare_bands: [Option<usize>; 8],
    #[serde(rename = "snare_window", skip_serializing)]
    legacy_snare_window: f32,
    #[serde(rename = "snare_min_onsets", skip_serializing)]
    legacy_snare_min_onsets: usize,
    #[serde(rename = "snare_min_power", skip_serializing)]
    legacy_snare_min_power: f32,
}

impl Default for DetectorSettingsRaw {
    fn default() -> Self {
        let mut raw: Self = DetectorSettings::default().into();
        raw.band_onset.clear();
        raw.legacy_band_flux.clear();
        raw.kick_pattern = None;
        raw.snare_pattern = None;
        raw.hat_pattern = None;
        raw.legacy_kick_kernel = None;
        raw.legacy_snare_kernel = None;
        raw
    }
}

impl TryFrom<DetectorSettingsRaw> for DetectorSettings {
    type Error = anyhow::Error;

    fn try_from(raw: DetectorSettingsRaw) -> Result<Self, Self::Error> {
        let band_onset: [BandOnsetSettings; BAND_COUNT] = if raw.band_onset.is_empty() {
            [BandOnsetSettings::default(); BAND_COUNT]
        } else {
            anyhow::ensure!(
                raw.band_onset.len() == BAND_COUNT,
                "band_onset must describe all {BAND_COUNT} bands"
            );
            raw.band_onset
                .try_into()
                .expect("the band count was checked above")
        };
        let kick_pattern = if let Some(pattern) = raw.kick_pattern {
            pattern.into_pattern()
        } else if let Some(kernel) = raw.legacy_kick_kernel {
            validate_kernel("kick", &kernel)?;
            TemporalPattern::from_legacy_steps(&[kernel])
        } else {
            TemporalPattern::kick_initial()
        };
        let snare_pattern = if let Some(pattern) = raw.snare_pattern {
            pattern.into_pattern()
        } else if let Some(kernel) = raw.legacy_snare_kernel {
            validate_kernel("snare", &kernel)?;
            TemporalPattern::from_legacy_steps(&[kernel])
        } else {
            TemporalPattern::snare_initial()
        };
        let mut kick_pattern = kick_pattern;
        if kick_pattern.suppression_inner.len() < 3 || kick_pattern.suppression_outer.len() < 3 {
            let initial = TemporalPattern::kick_initial();
            kick_pattern.suppression_inner = initial.suppression_inner;
            kick_pattern.suppression_outer = initial.suppression_outer;
        }
        let mut snare_pattern = snare_pattern;
        if snare_pattern.suppression_inner.len() < 3 || snare_pattern.suppression_outer.len() < 3 {
            let initial = TemporalPattern::snare_initial();
            snare_pattern.suppression_inner = initial.suppression_inner;
            snare_pattern.suppression_outer = initial.suppression_outer;
        }
        let mut hat_pattern = raw.hat_pattern.map_or_else(
            TemporalPattern::hat_initial,
            TemporalPatternRaw::into_pattern,
        );
        if hat_pattern.suppression_inner.len() < 3 || hat_pattern.suppression_outer.len() < 3 {
            let initial = TemporalPattern::hat_initial();
            hat_pattern.suppression_inner = initial.suppression_inner;
            hat_pattern.suppression_outer = initial.suppression_outer;
        }
        let drop_gap_threshold = if raw.legacy_drop_gap_window.is_some() {
            DetectorSettings::default().drop_gap_threshold
        } else {
            raw.drop_gap_threshold
        };
        Self {
            band_onset,
            excess: raw.excess,
            kick_pattern,
            kick_onset_pre_gain: raw.kick_onset_pre_gain,
            kick_onset_attack: raw.kick_onset_attack,
            kick_onset_release: raw.kick_onset_release,
            kick_onset_average: raw.kick_onset_average,
            kick_onset_sensitivity: raw.kick_onset_sensitivity,
            kick_onset_min_threshold: raw.kick_onset_min_threshold,
            kick_onset_min_interval: raw.kick_onset_min_interval,
            kick_onset_suppression: raw.kick_onset_suppression,
            kick_snare_choke: raw.kick_snare_choke,
            kick_drop_choke: raw.kick_drop_choke,
            kick_quake_factor: raw.kick_quake_factor,
            kick_sequence_every: raw.kick_sequence_every,
            snare_pattern,
            snare_onset_pre_gain: raw.snare_onset_pre_gain,
            snare_onset_attack: raw.snare_onset_attack,
            snare_onset_release: raw.snare_onset_release,
            snare_onset_average: raw.snare_onset_average,
            snare_onset_sensitivity: raw.snare_onset_sensitivity,
            snare_onset_min_threshold: raw.snare_onset_min_threshold,
            snare_onset_min_interval: raw.snare_onset_min_interval,
            snare_onset_suppression: raw.snare_onset_suppression,
            snare_min_rms_db: raw.snare_min_rms_db,
            snare_flip_every: raw.snare_flip_every,
            snare_poi_switches: raw.snare_poi_switches,
            snare_choke: raw.snare_choke,
            flip_stiffness: raw.flip_stiffness,
            flip_damping: raw.flip_damping,
            hat_pattern,
            hat_onset_pre_gain: raw.hat_onset_pre_gain,
            hat_onset_attack: raw.hat_onset_attack,
            hat_onset_release: raw.hat_onset_release,
            hat_onset_average: raw.hat_onset_average,
            hat_onset_sensitivity: raw.hat_onset_sensitivity,
            hat_onset_min_threshold: raw.hat_onset_min_threshold,
            hat_onset_min_interval: raw.hat_onset_min_interval,
            hat_onset_suppression: raw.hat_onset_suppression,
            drop_kernels: DropKernels {
                onset: raw.drop_onset_kernel,
            },
            drop_gap_threshold,
            drop_onset_pre_gain: raw.drop_onset_pre_gain,
            drop_onset_attack: raw.drop_onset_attack,
            drop_onset_release: raw.drop_onset_release,
            drop_onset_average: raw.drop_onset_average,
            drop_onset_sensitivity: raw.drop_onset_sensitivity,
            drop_onset_min_threshold: raw.drop_onset_min_threshold,
            drop_onset_min_interval: raw.drop_onset_min_interval,
            drop_onset_suppression: raw.drop_onset_suppression,
            drop_quake_factor: raw.drop_quake_factor,
            quake_stiffness: raw.quake_stiffness,
            quake_damping: raw.quake_damping,
        }
        .validate()
    }
}

impl From<DetectorSettings> for DetectorSettingsRaw {
    fn from(settings: DetectorSettings) -> Self {
        Self {
            band_onset: settings.band_onset.to_vec(),
            excess: settings.excess,
            legacy_band_flux: Vec::new(),
            legacy_flux_attack: 0.0,
            legacy_flux_release: 0.0,
            legacy_flux_average: 0.0,
            legacy_flux_sensitivity: 0.0,
            legacy_flux_min_threshold: 0.0,
            legacy_flux_min_interval: 0.0,
            kick_pattern: Some(settings.kick_pattern.clone().into()),
            legacy_kick_kernel: Some(std::array::from_fn(|band| if band < 5 { 1.0 } else { 0.0 })),
            kick_onset_pre_gain: settings.kick_onset_pre_gain,
            kick_onset_attack: settings.kick_onset_attack,
            kick_onset_release: settings.kick_onset_release,
            kick_onset_average: settings.kick_onset_average,
            kick_onset_sensitivity: settings.kick_onset_sensitivity,
            kick_onset_min_threshold: settings.kick_onset_min_threshold,
            kick_onset_min_interval: settings.kick_onset_min_interval,
            kick_onset_suppression: settings.kick_onset_suppression,
            kick_snare_choke: settings.kick_snare_choke,
            kick_drop_choke: settings.kick_drop_choke,
            kick_quake_factor: settings.kick_quake_factor,
            kick_sequence_every: settings.kick_sequence_every,
            snare_pattern: Some(settings.snare_pattern.clone().into()),
            legacy_snare_kernel: Some(std::array::from_fn(|band| {
                if matches!(band, 5 | 6 | 10 | 12) {
                    1.0
                } else {
                    0.0
                }
            })),
            snare_onset_pre_gain: settings.snare_onset_pre_gain,
            snare_onset_attack: settings.snare_onset_attack,
            snare_onset_release: settings.snare_onset_release,
            snare_onset_average: settings.snare_onset_average,
            snare_onset_sensitivity: settings.snare_onset_sensitivity,
            snare_onset_min_threshold: settings.snare_onset_min_threshold,
            snare_onset_min_interval: settings.snare_onset_min_interval,
            snare_onset_suppression: settings.snare_onset_suppression,
            snare_min_rms_db: settings.snare_min_rms_db,
            snare_flip_every: settings.snare_flip_every,
            snare_poi_switches: settings.snare_poi_switches,
            snare_choke: settings.snare_choke,
            flip_stiffness: settings.flip_stiffness,
            flip_damping: settings.flip_damping,
            hat_pattern: Some(settings.hat_pattern.clone().into()),
            hat_onset_pre_gain: settings.hat_onset_pre_gain,
            hat_onset_attack: settings.hat_onset_attack,
            hat_onset_release: settings.hat_onset_release,
            hat_onset_average: settings.hat_onset_average,
            hat_onset_sensitivity: settings.hat_onset_sensitivity,
            hat_onset_min_threshold: settings.hat_onset_min_threshold,
            hat_onset_min_interval: settings.hat_onset_min_interval,
            hat_onset_suppression: settings.hat_onset_suppression,
            drop_gap_threshold: settings.drop_gap_threshold,
            drop_onset_pre_gain: settings.drop_onset_pre_gain,
            drop_onset_attack: settings.drop_onset_attack,
            drop_onset_release: settings.drop_onset_release,
            drop_onset_average: settings.drop_onset_average,
            drop_onset_sensitivity: settings.drop_onset_sensitivity,
            drop_onset_min_threshold: settings.drop_onset_min_threshold,
            drop_onset_min_interval: settings.drop_onset_min_interval,
            drop_onset_suppression: settings.drop_onset_suppression,
            drop_onset_kernel: settings.drop_kernels.onset,
            drop_quake_factor: settings.drop_quake_factor,
            quake_stiffness: settings.quake_stiffness,
            quake_damping: settings.quake_damping,
            legacy_drop_split: 0,
            legacy_drop_onsets: 0,
            legacy_drop_window: 0.0,
            legacy_drop_gap_window: None,
            legacy_drop_gap_gain: 0.0,
            legacy_drop_low_lp: 0.0,
            legacy_drop_high_lp: 0.0,
            legacy_drop_low_kernel: [0.0; BAND_COUNT],
            legacy_drop_high_kernel: [0.0; BAND_COUNT],
            legacy_drop_gap_level: 0.0,
            legacy_drop_gap_time: 0.0,
            legacy_drop_quiet_level: 0.0,
            legacy_drop_quiet_time: 0.0,
            legacy_kick_low: 0,
            legacy_kick_high: 0,
            legacy_kick_window: 0.0,
            legacy_snare_bands: [None; 8],
            legacy_snare_window: 0.0,
            legacy_snare_min_onsets: 0,
            legacy_snare_min_power: 0.0,
        }
    }
}

/// Onsets produced by one analysis hop. Only the composite detectors report
/// boolean onsets; the per-band signal is a continuous flow.
#[derive(Clone, Copy, Debug, Default)]
pub struct DetectorEvents {
    pub kick: Option<f32>,
    pub snare: Option<f32>,
    pub hat: Option<f32>,
    pub drop: Option<f32>,
    /// The gap gate crossed from unarmed to armed on this hop.
    pub gap_entry: bool,
}

/// One onset tracker recipe. Each composite detector keeps its own copy for
/// the kernel-projected flow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FluxSettings {
    pub attack: f32,
    pub release: f32,
    /// Reference memory in seconds: the time constant of the flux-level
    /// average and the window of the peak median.
    pub average: f32,
    /// Threshold as a fraction of the reference.
    pub sensitivity: f32,
    /// Absolute threshold floor. The peak median also admits only peaks above
    /// this value.
    pub min_threshold: f32,
    pub min_interval: f32,
}

impl Default for FluxSettings {
    fn default() -> Self {
        Self {
            attack: 0.005,
            release: 0.05,
            average: 0.2,
            sensitivity: 1.5,
            min_threshold: 0.01,
            min_interval: 0.14,
        }
    }
}

/// One composite detector's onset recipe: the kernel that excites it, the
/// weight of the inverted-kernel inhibition, and the tracker that turns the
/// residual flow into onsets.
#[derive(Clone, Copy, Debug)]
struct OnsetRecipe {
    kernel: [f32; BAND_COUNT],
    suppression: f32,
    pre_gain: f32,
    tracker: FluxSettings,
}

/// One composite projection: the inverted-kernel flow, the part of it the
/// suppression weight subtracts, and the residual that reaches the tracker.
#[derive(Clone, Copy, Debug, Default)]
struct OnsetProjection {
    /// Inverted-kernel weighted onset flow after pre-gain, before suppression.
    inhibition: f32,
    /// Inhibition after the detector's suppression weight.
    suppressed: f32,
    /// Excitation minus the weighted inhibition, clamped at zero.
    residual: f32,
}

impl OnsetRecipe {
    /// Projects the per-band onset flow onto the kernel and subtracts the flow
    /// selected by the inverted kernel. The residual is the transient that
    /// belongs to this instrument rather than to the rest of the mix.
    fn project(&self, tracked: &[f32; BAND_COUNT]) -> OnsetProjection {
        let excited = kernel_projection(&self.kernel, tracked) * self.pre_gain;
        let inverted: [f32; BAND_COUNT] = std::array::from_fn(|band| 1.0 - self.kernel[band]);
        let inhibition = kernel_projection(&inverted, tracked) * self.pre_gain;
        let suppressed = self.suppression * inhibition;
        OnsetProjection {
            inhibition,
            suppressed,
            residual: (excited - suppressed).max(0.0),
        }
    }
}

/// One peak of the smoothed onset flow and the time it was seen.
#[derive(Clone, Copy, Debug)]
struct OnsetPeak {
    at: f32,
    amplitude: f32,
}

/// Median amplitude of the onset peaks inside a rolling time window.
///
/// The median ignores a single loud crash, so the threshold follows the
/// track's typical onset amplitude instead of its loudest moment. When the
/// window is empty the tracker falls back to its absolute threshold.
#[derive(Debug)]
struct PeakMedianWindow {
    peaks: VecDeque<OnsetPeak>,
    scratch: Vec<f32>,
    median: Option<f32>,
}

impl Default for PeakMedianWindow {
    fn default() -> Self {
        Self {
            peaks: VecDeque::with_capacity(PEAK_WINDOW_CAPACITY),
            scratch: Vec::with_capacity(PEAK_WINDOW_CAPACITY),
            median: None,
        }
    }
}

impl PeakMedianWindow {
    /// Drops the peaks that left the window and records the peak that appeared
    /// on this hop.
    fn advance(&mut self, now: f32, peak_at: f32, window: f32, candidate: Option<f32>) {
        let mut changed = self.prune(now, window);
        if let Some(amplitude) = candidate {
            self.peaks.push_back(OnsetPeak {
                at: peak_at,
                amplitude,
            });
            changed = true;
        }
        if changed {
            self.update();
        }
    }

    fn prune(&mut self, now: f32, window: f32) -> bool {
        let mut changed = false;
        while let Some(peak) = self.peaks.front() {
            if now - peak.at <= window {
                break;
            }
            self.peaks.pop_front();
            changed = true;
        }
        changed
    }

    fn update(&mut self) {
        if self.peaks.is_empty() {
            self.median = None;
            return;
        }
        self.scratch.clear();
        self.scratch
            .extend(self.peaks.iter().map(|peak| peak.amplitude));
        let middle = self.scratch.len() / 2;
        let (_, median, _) = self.scratch.select_nth_unstable_by(middle, f32::total_cmp);
        self.median = Some(*median);
    }

    fn clear(&mut self) {
        self.peaks.clear();
        self.median = None;
    }
}

/// Where a composite detector's onset threshold comes from.
#[derive(Debug)]
enum OnsetReference {
    /// Slow mean of the smoothed flow. The drop detector keeps this reference
    /// until its own onset story is revisited.
    FluxLevel(f32),
    /// Median of the recent onset peaks. Kick, snare, and hi-hat use this
    /// reference so the threshold follows the track's own onset amplitude.
    PeakMedian(PeakMedianWindow),
}

impl OnsetReference {
    fn value(&self) -> Option<f32> {
        match self {
            Self::FluxLevel(average) => Some(*average),
            Self::PeakMedian(window) => window.median,
        }
    }

    fn advance(
        &mut self,
        smoothed: f32,
        now: f32,
        dt: f32,
        settings: &FluxSettings,
        candidate: Option<f32>,
    ) {
        match self {
            Self::FluxLevel(average) => {
                let alpha = (-dt / settings.average).exp();
                *average = alpha * *average + (1.0 - alpha) * smoothed;
            }
            Self::PeakMedian(window) => {
                window.advance(now, now - dt, settings.average, candidate);
            }
        }
    }

    /// Strength of a confirmed onset relative to the reference that judged it.
    fn power(&self, peak: f32, threshold: f32) -> f32 {
        match self {
            // The drop detector keeps reporting strength over its threshold.
            Self::FluxLevel(_) => peak / threshold.max(1e-6),
            // A track-relative strength of one is the typical onset amplitude.
            Self::PeakMedian(window) => match window.median {
                Some(reference) if reference > 0.0 => peak / reference,
                _ => 1.0,
            },
        }
    }

    fn reset(&mut self) {
        match self {
            Self::FluxLevel(average) => *average = 0.0,
            Self::PeakMedian(window) => window.clear(),
        }
    }
}

/// One composite detector's onset tracker.
///
/// The attack/release envelope turns the projected flow into a peaky signal.
/// The threshold is `max(reference * sensitivity, min_threshold)`. The peak
/// test uses the threshold from the previous hop, so a peak cannot raise its
/// own bar.
#[derive(Debug)]
struct FluxTracker {
    raw: f32,
    smoothed: f32,
    reference: OnsetReference,
    previous_smoothed: f32,
    before_previous_smoothed: f32,
    threshold: f32,
    last_onset: f32,
    primed: bool,
}

impl Default for FluxTracker {
    /// Kick, snare, and hi-hat start on the peak-median reference.
    fn default() -> Self {
        Self::with_reference(OnsetReference::PeakMedian(PeakMedianWindow::default()))
    }
}

impl FluxTracker {
    /// The drop detector keeps the flux-level reference.
    fn level() -> Self {
        Self::with_reference(OnsetReference::FluxLevel(0.0))
    }

    fn with_reference(reference: OnsetReference) -> Self {
        Self {
            raw: 0.0,
            smoothed: 0.0,
            reference,
            previous_smoothed: 0.0,
            before_previous_smoothed: 0.0,
            threshold: 0.0,
            last_onset: 0.0,
            primed: false,
        }
    }

    fn observe(&mut self, flux: f32, now: f32, dt: f32, settings: &FluxSettings) -> Option<f32> {
        self.raw = flux;
        if !self.primed {
            self.smoothed = flux;
            self.previous_smoothed = flux;
            self.before_previous_smoothed = flux;
            if let OnsetReference::FluxLevel(average) = &mut self.reference {
                *average = flux;
            }
            self.threshold = self.reference_threshold(settings);
            self.last_onset = f32::NEG_INFINITY;
            self.primed = true;
            return None;
        }
        let attack_alpha = (-dt / settings.attack).exp();
        let release_alpha = (-dt / settings.release).exp();
        let alpha = if flux > self.smoothed {
            attack_alpha
        } else {
            release_alpha
        };
        self.smoothed = alpha * self.smoothed + (1.0 - alpha) * flux;

        let previous = self.previous_smoothed;
        let threshold = self.threshold;
        let local_peak = previous >= self.before_previous_smoothed && previous > self.smoothed;
        let onset = local_peak
            && previous >= threshold
            && now - dt - self.last_onset >= settings.min_interval;
        let power = if onset {
            self.last_onset = now - dt;
            Some(self.reference.power(previous, threshold))
        } else {
            None
        };
        let candidate = (local_peak && previous >= settings.min_threshold).then_some(previous);
        self.reference
            .advance(self.smoothed, now, dt, settings, candidate);
        self.threshold = self.reference_threshold(settings);
        self.before_previous_smoothed = previous;
        self.previous_smoothed = self.smoothed;
        power
    }

    fn reference_threshold(&self, settings: &FluxSettings) -> f32 {
        let reference = self.reference.value().unwrap_or(0.0);
        (reference * settings.sensitivity).max(settings.min_threshold)
    }

    fn reset(&mut self) {
        self.raw = 0.0;
        self.smoothed = 0.0;
        self.reference.reset();
        self.previous_smoothed = 0.0;
        self.before_previous_smoothed = 0.0;
        self.threshold = 0.0;
        self.last_onset = 0.0;
        self.primed = false;
    }
}

#[derive(Debug, Default)]
struct KickTracker {
    onset: FluxTracker,
    choked_until: f32,
    flash: f32,
    last_power: f32,
}

impl KickTracker {
    fn observe(&mut self, flux: f32, now: f32, dt: f32, settings: &FluxSettings) -> Option<f32> {
        self.flash *= (-dt * FLASH_DECAY).exp();
        let onset = self.onset.observe(flux, now, dt, settings);
        if now < self.choked_until {
            return None;
        }
        let power = onset?;
        self.flash = 1.0;
        self.last_power = power;
        Some(power)
    }

    fn choke(&mut self, now: f32, duration: f32) {
        self.choked_until = self.choked_until.max(now + duration);
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
struct SnareTracker {
    onset: FluxTracker,
    flash: f32,
    last_power: f32,
}

impl SnareTracker {
    /// Runs the onset tracker without emitting a trigger; the caller decides
    /// whether the absolute gate lets the trigger through.
    fn observe(&mut self, flux: f32, now: f32, dt: f32, settings: &FluxSettings) -> Option<f32> {
        self.flash *= (-dt * FLASH_DECAY).exp();
        self.onset.observe(flux, now, dt, settings)
    }

    /// Publishes a trigger that passed its gate.
    fn emit(&mut self, power: f32) {
        self.flash = 1.0;
        self.last_power = power;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Converts a linear RMS value to dBFS, clamped at the silence floor.
fn rms_to_db(rms: f32) -> f32 {
    (20.0 * rms.max(1e-9).log10()).max(GAP_DB_FLOOR)
}

#[derive(Debug)]
struct DropTracker {
    onset: FluxTracker,
    /// Smoothed linear RMS.
    envelope: f32,
    envelope_ready: bool,
    rms_db: f32,
    envelope_db: f32,
    /// Latches high while the envelope is below the gap gate; a composite
    /// onset clears it.
    eligible: bool,
    /// Whether this hop crossed from unarmed to armed.
    gap_entry: bool,
    flash: f32,
    last_power: f32,
}

impl Default for DropTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DropTracker {
    fn new() -> Self {
        Self {
            onset: FluxTracker::level(),
            envelope: 0.0,
            envelope_ready: false,
            rms_db: GAP_DB_FLOOR,
            envelope_db: GAP_DB_FLOOR,
            eligible: false,
            gap_entry: false,
            flash: 0.0,
            last_power: 0.0,
        }
    }

    fn observe(
        &mut self,
        now: f32,
        rms: f32,
        onset_flux: f32,
        settings: &DetectorSettings,
        dt: f32,
    ) -> Option<f32> {
        self.flash *= (-dt * FLASH_DECAY).exp();
        self.rms_db = rms_to_db(rms);
        if self.envelope_ready {
            let tau = if rms > self.envelope {
                GAP_ENVELOPE_ATTACK
            } else {
                GAP_ENVELOPE_RELEASE
            };
            let alpha = (-dt / tau).exp();
            self.envelope = alpha * self.envelope + (1.0 - alpha) * rms;
        } else {
            self.envelope = rms;
            self.envelope_ready = true;
        }
        self.envelope_db = rms_to_db(self.envelope);
        // The gate latches below the threshold: the confirming onset usually
        // arrives after the level has already started to rise again.
        let was_eligible = self.eligible;
        self.eligible |= self.envelope_db < settings.drop_gap_threshold;
        self.gap_entry = !was_eligible && self.eligible;

        let onset_settings = settings.drop_onset_recipe().tracker;
        let power = self.onset.observe(onset_flux, now, dt, &onset_settings)?;
        if !self.eligible {
            return None;
        }
        self.eligible = false;
        self.flash = 1.0;
        self.last_power = power;
        Some(power)
    }

    fn reset(&mut self) {
        self.onset.reset();
        self.envelope = 0.0;
        self.envelope_ready = false;
        self.rms_db = GAP_DB_FLOOR;
        self.envelope_db = GAP_DB_FLOOR;
        self.eligible = false;
        self.gap_entry = false;
        self.flash = 0.0;
        self.last_power = 0.0;
    }
}

/// All per-band trackers and the four confirmed detectors. One `observe` call
/// advances the state by one analysis hop.
pub struct OnsetBank {
    band_excess: [f32; BAND_COUNT],
    band_abdr: [f32; BAND_COUNT],
    band_abl: [f32; BAND_COUNT],
    band_drp: [f32; BAND_COUNT],
    band_onset: [f32; BAND_COUNT],
    /// Newest per-hop onset flows, used by the temporal kick, snare, and
    /// hi-hat templates.
    onset_history: VecDeque<[f32; BAND_COUNT]>,
    kick: KickTracker,
    snare: SnareTracker,
    hat: SnareTracker,
    drop: DropTracker,
    kick_projection: OnsetProjection,
    snare_projection: OnsetProjection,
    hat_projection: OnsetProjection,
    drop_projection: OnsetProjection,
    time: f32,
}

impl Default for OnsetBank {
    fn default() -> Self {
        Self::new()
    }
}

impl OnsetBank {
    pub fn new() -> Self {
        Self {
            band_excess: [0.0; BAND_COUNT],
            band_abdr: [0.0; BAND_COUNT],
            band_abl: [0.0; BAND_COUNT],
            band_drp: [0.0; BAND_COUNT],
            band_onset: [0.0; BAND_COUNT],
            onset_history: VecDeque::with_capacity(PATTERN_MAX_STEPS),
            kick: KickTracker::default(),
            snare: SnareTracker::default(),
            hat: SnareTracker::default(),
            drop: DropTracker::default(),
            kick_projection: OnsetProjection::default(),
            snare_projection: OnsetProjection::default(),
            hat_projection: OnsetProjection::default(),
            drop_projection: OnsetProjection::default(),
            time: 0.0,
        }
    }

    /// Feeds one analysis hop. The continuous per-band onset flow is appended
    /// to the pattern history; kick, snare, and hi-hat match their temporal
    /// templates
    /// against it, while drop projects the current flow through its onset
    /// kernel. Each composite detector then tracks its own onsets.
    pub fn observe(
        &mut self,
        spectrum: &Spectrum,
        band_abdr: &[f32; BAND_COUNT],
        band_abl: &[f32; BAND_COUNT],
        settings: &DetectorSettings,
        dt: f32,
    ) -> DetectorEvents {
        self.time += dt;
        let wsdr = band_abdr.iter().sum::<f32>() / BAND_COUNT as f32;
        for band in 0..BAND_COUNT {
            let drp = if wsdr > 1e-6 {
                band_abdr[band] / wsdr
            } else {
                0.0
            };
            self.band_excess[band] = spectrum.band_excess[band];
            self.band_abdr[band] = band_abdr[band];
            self.band_abl[band] = band_abl[band];
            self.band_drp[band] = drp;
            self.band_onset[band] = spectrum.band_excess[band] * drp * band_abl[band];
        }
        if self.onset_history.len() == PATTERN_MAX_STEPS {
            self.onset_history.pop_front();
        }
        self.onset_history.push_back(self.band_onset);
        let tracked = self.band_onset;
        let pattern_steps = (0.5 / dt).floor().max(1.0) as usize;

        let mut events = DetectorEvents::default();
        let snare_projection = settings.snare_pattern.project(
            &self.onset_history,
            settings.snare_onset_suppression,
            settings.snare_onset_pre_gain,
            pattern_steps,
        );
        self.snare_projection = snare_projection;
        let rms_db = rms_to_db(spectrum.rms);
        if let Some(snare) = self.snare.observe(
            snare_projection.residual,
            self.time,
            dt,
            &settings.snare_onset_tracker(),
        ) {
            if rms_db >= settings.snare_min_rms_db {
                self.snare.emit(snare);
                events.snare = Some(snare);
                self.kick.choke(self.time, settings.snare_choke);
            }
        }
        let hat_projection = settings.hat_pattern.project(
            &self.onset_history,
            settings.hat_onset_suppression,
            settings.hat_onset_pre_gain,
            pattern_steps,
        );
        self.hat_projection = hat_projection;
        if let Some(hat) = self.hat.observe(
            hat_projection.residual,
            self.time,
            dt,
            &settings.hat_onset_tracker(),
        ) {
            self.hat.emit(hat);
            events.hat = Some(hat);
        }
        let drop_recipe = settings.drop_onset_recipe();
        let drop_projection = drop_recipe.project(&tracked);
        self.drop_projection = drop_projection;
        let drop = self.drop.observe(
            self.time,
            spectrum.rms,
            drop_projection.residual,
            settings,
            dt,
        );
        events.gap_entry = self.drop.gap_entry;
        if let Some(drop) = drop {
            events.drop = Some(drop);
            self.kick.choke(self.time, settings.kick_drop_choke);
        }
        let kick_projection = settings.kick_pattern.project(
            &self.onset_history,
            settings.kick_onset_suppression,
            settings.kick_onset_pre_gain,
            pattern_steps,
        );
        self.kick_projection = kick_projection;
        events.kick = self.kick.observe(
            kick_projection.residual,
            self.time,
            dt,
            &settings.kick_onset_tracker(),
        );
        events
    }

    pub fn reset(&mut self) {
        self.band_excess = [0.0; BAND_COUNT];
        self.band_abdr = [0.0; BAND_COUNT];
        self.band_abl = [0.0; BAND_COUNT];
        self.band_drp = [0.0; BAND_COUNT];
        self.band_onset = [0.0; BAND_COUNT];
        self.onset_history.clear();
        self.kick.reset();
        self.snare.reset();
        self.hat.reset();
        self.drop.reset();
        self.kick_projection = OnsetProjection::default();
        self.snare_projection = OnsetProjection::default();
        self.hat_projection = OnsetProjection::default();
        self.drop_projection = OnsetProjection::default();
        self.time = 0.0;
    }

    pub fn onset_history(&self) -> &VecDeque<[f32; BAND_COUNT]> {
        &self.onset_history
    }

    pub fn status(&self) -> DetectionStatus {
        DetectionStatus {
            bands: std::array::from_fn(|index| BandStatus {
                excess: self.band_excess[index],
                onset: self.band_onset[index],
                abdr: self.band_abdr[index],
                abl: self.band_abl[index],
                drp: self.band_drp[index],
            }),
            kick: TriggerStatus {
                flash: self.kick.flash,
                power: self.kick.last_power,
                count: 0,
                armed: false,
                choked: self.time < self.kick.choked_until,
            },
            snare: TriggerStatus {
                flash: self.snare.flash,
                power: self.snare.last_power,
                count: 0,
                armed: false,
                choked: false,
            },
            hat: TriggerStatus {
                flash: self.hat.flash,
                power: self.hat.last_power,
                count: 0,
                armed: false,
                choked: false,
            },
            drop: TriggerStatus {
                flash: self.drop.flash,
                power: self.drop.last_power,
                count: 0,
                armed: self.drop.eligible,
                choked: false,
            },
            kick_flux: onset_flux_status(&self.kick.onset, self.kick_projection),
            snare_flux: onset_flux_status(&self.snare.onset, self.snare_projection),
            hat_flux: onset_flux_status(&self.hat.onset, self.hat_projection),
            drop_status: DropStatus {
                rms_db: self.drop.rms_db,
                envelope_db: self.drop.envelope_db,
                onset: onset_flux_status(&self.drop.onset, self.drop_projection),
            },
        }
    }
}

fn onset_flux_status(tracker: &FluxTracker, projection: OnsetProjection) -> OnsetFluxStatus {
    OnsetFluxStatus {
        flux: tracker.raw,
        smoothed: tracker.smoothed,
        reference: tracker.reference.value(),
        threshold: tracker.threshold,
        inhibition: projection.inhibition,
        suppressed: projection.suppressed,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BandStatus {
    /// Raw relative spectral excess for this band.
    pub excess: f32,
    /// Continuous onset flow after DRP and ABL scaling.
    pub onset: f32,
    /// Tracked high-minus-low reference distance.
    pub abdr: f32,
    /// Absolute band loudness derived from the high reference.
    pub abl: f32,
    /// Dynamic-range parity, ABDR / WSDR.
    pub drp: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct TriggerStatus {
    pub flash: f32,
    pub power: f32,
    pub count: u32,
    pub armed: bool,
    pub choked: bool,
}

impl TriggerStatus {
    pub const ZERO: Self = Self {
        flash: 0.0,
        power: 0.0,
        count: 0,
        armed: false,
        choked: false,
    };
}

/// Drop state behind the trigger: the RMS level and its envelope, the gap gate,
/// and the composite onset tracker.
#[derive(Clone, Copy, Debug, Default)]
pub struct DropStatus {
    /// Analysis-window RMS in dBFS.
    pub rms_db: f32,
    /// Smoothed RMS envelope in dBFS.
    pub envelope_db: f32,
    pub onset: OnsetFluxStatus,
}

/// Raw, smoothed, and adaptive-threshold values from one composite onset
/// tracker, plus the suppression flux it subtracts.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnsetFluxStatus {
    pub flux: f32,
    pub smoothed: f32,
    /// Median of the recent onset peaks, or the slow flux level for the drop
    /// detector. Threshold is derived from it; `None` before the reference has
    /// any data.
    pub reference: Option<f32>,
    pub threshold: f32,
    /// Inverted-kernel weighted tracked flux before the suppression weight.
    pub inhibition: f32,
    /// What the suppression weight subtracts from the excitation.
    pub suppressed: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DetectionStatus {
    pub bands: [BandStatus; BAND_COUNT],
    pub kick: TriggerStatus,
    pub snare: TriggerStatus,
    pub hat: TriggerStatus,
    pub drop: TriggerStatus,
    pub kick_flux: OnsetFluxStatus,
    pub snare_flux: OnsetFluxStatus,
    pub hat_flux: OnsetFluxStatus,
    pub drop_status: DropStatus,
}

/// One axis and direction of the sequenced quake impulse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuakeDirection {
    Left,
    Top,
    Right,
    Bottom,
}

impl QuakeDirection {
    const CYCLE: [Self; 4] = [Self::Left, Self::Top, Self::Right, Self::Bottom];

    /// Screen-space unit vector for this direction.
    pub fn unit(self) -> (f32, f32) {
        match self {
            Self::Left => (-1.0, 0.0),
            Self::Top => (0.0, -1.0),
            Self::Right => (1.0, 0.0),
            Self::Bottom => (0.0, 1.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Top => "TOP",
            Self::Right => "RIGHT",
            Self::Bottom => "BOTTOM",
        }
    }
}

/// Advances the quake direction every Nth kick and restarts on a snare.
#[derive(Clone, Copy, Debug, Default)]
pub struct QuakeSequence {
    count: u32,
    cursor: usize,
}

impl QuakeSequence {
    pub fn next(&mut self, every: u32) -> QuakeDirection {
        let every = every.max(1);
        self.count += 1;
        if self.count.is_multiple_of(every) {
            let direction = QuakeDirection::CYCLE[self.cursor];
            self.cursor = (self.cursor + 1) % QuakeDirection::CYCLE.len();
            direction
        } else {
            QuakeDirection::CYCLE[self.cursor]
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// A damped spring used by the flip and quake actions.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
}

impl Spring {
    pub fn value(&self) -> f32 {
        self.value
    }

    #[cfg(test)]
    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub fn impulse(&mut self, velocity: f32) {
        self.velocity += velocity;
    }

    pub fn advance(&mut self, dt: f32, stiffness: f32, damping: f32) {
        assert!(stiffness > 0.0, "a spring needs a positive stiffness");
        let mut remaining = dt.max(0.0);
        while remaining > 0.0 {
            let step = remaining.min(SPRING_STEP);
            let acceleration = -stiffness * (self.value - self.target) - damping * self.velocity;
            self.velocity += acceleration * step;
            self.value += self.velocity * step;
            remaining -= step;
        }
    }

    pub fn reset(&mut self, value: f32) {
        self.value = value;
        self.velocity = 0.0;
        self.target = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> DetectorSettings {
        DetectorSettings::default()
    }

    /// Peak-tracking settings with the fast envelope of the real detectors.
    fn adaptive_settings() -> FluxSettings {
        FluxSettings {
            attack: 0.001,
            release: 0.005,
            average: 2.0,
            sensitivity: 0.5,
            min_threshold: 0.01,
            min_interval: 0.05,
        }
    }

    /// One hit followed by seven silent hops, so pulses stay independent.
    fn pulse_train(amplitude: f32, hits: usize) -> Vec<f32> {
        let mut flux = Vec::new();
        for _ in 0..hits {
            flux.push(amplitude);
            flux.extend(std::iter::repeat_n(0.0, 7));
        }
        flux
    }

    fn feed(tracker: &mut FluxTracker, settings: &FluxSettings, train: &[f32]) -> Vec<f32> {
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut onsets = Vec::new();
        for &flux in train {
            now += dt;
            if let Some(power) = tracker.observe(flux, now, dt, settings) {
                onsets.push(power);
            }
        }
        onsets
    }

    #[test]
    fn abl_scales_the_high_reference_before_the_curve() {
        let settings = BandOnsetSettings {
            abl_gain: 2.0,
            abl_curve: Curve::Linear,
            abl_amount: 1.0,
        };
        assert!((settings.loudness(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(BandOnsetSettings::default().loudness(1.0), 1.0);
        assert_eq!(BandOnsetSettings::default().loudness(0.0), 0.0);
    }

    #[test]
    fn band_onset_defaults_match_the_recipe() {
        let settings = defaults();
        let band = settings.band_onset[0];
        assert_eq!(band.abl_gain, 1.0);
        assert_eq!(band.abl_curve, Curve::SCurve);
        assert_eq!(band.abl_amount, 2.5);
        assert_eq!(settings.excess.background_attack, 0.320);
        assert_eq!(settings.excess.background_release, 0.200);
        assert_eq!(settings.excess.floor, 0.010);
        assert_eq!(settings.excess.cap, 1.0);
        assert!(settings
            .band_onset
            .iter()
            .all(|candidate| *candidate == band));
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn a_flux_peak_triggers_after_the_local_maximum() {
        let settings = FluxSettings::default();
        let mut tracker = FluxTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut fired = false;
        for flux in [0.0, 0.0, 0.5, 1.0, 0.8, 0.2, 0.0, 0.0] {
            now += dt;
            if tracker.observe(flux, now, dt, &settings).is_some() {
                fired = true;
            }
        }
        assert!(fired, "a flux pulse must produce one local-peak onset");
    }

    #[test]
    fn a_steady_flux_does_not_trigger() {
        let settings = FluxSettings::default();
        let mut tracker = FluxTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..120 {
            now += dt;
            assert!(tracker.observe(0.5, now, dt, &settings).is_none());
        }
    }

    #[test]
    fn repeated_flux_peaks_respect_the_minimum_interval() {
        let flux_settings = FluxSettings {
            min_interval: 0.05,
            ..FluxSettings::default()
        };
        let mut tracker = FluxTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut fires = 0;
        for flux in [0.0, 0.0, 1.0, 0.0, 1.0, 0.0] {
            now += dt;
            if tracker.observe(flux, now, dt, &flux_settings).is_some() {
                fires += 1;
            }
        }
        assert_eq!(
            fires, 1,
            "the second pulse lands inside the minimum interval"
        );
    }

    #[test]
    fn the_peak_median_threshold_follows_the_track_scale() {
        let settings = adaptive_settings();
        let loud = feed(
            &mut FluxTracker::default(),
            &settings,
            &pulse_train(0.32, 12),
        );
        let quiet = feed(
            &mut FluxTracker::default(),
            &settings,
            &pulse_train(0.04, 12),
        );
        assert_eq!(loud.len(), 12);
        assert_eq!(
            quiet.len(),
            12,
            "a quiet track keeps the same hit count as a loud one"
        );
    }

    #[test]
    fn one_loud_peak_does_not_raise_the_median_threshold() {
        let settings = adaptive_settings();
        let mut flux = pulse_train(0.1, 8);
        flux.extend(pulse_train(1.0, 1));
        flux.extend(pulse_train(0.1, 8));
        let onsets = feed(&mut FluxTracker::default(), &settings, &flux);
        assert_eq!(
            onsets.len(),
            17,
            "the median must ignore the single loud crash"
        );
        for power in &onsets[9..] {
            assert!(
                (power - 1.0).abs() < 0.05,
                "the peaks after the crash stay at the median"
            );
        }
    }

    #[test]
    fn the_onset_power_is_relative_to_the_peak_median() {
        let settings = adaptive_settings();
        let mut flux = pulse_train(0.1, 8);
        flux.extend(pulse_train(0.06, 1));
        let onsets = feed(&mut FluxTracker::default(), &settings, &flux);
        assert_eq!(onsets.len(), 9);
        assert!(
            (onsets[8] - 0.6).abs() < 1e-3,
            "a 60% peak reports 60% of the median"
        );
    }

    #[test]
    fn the_peak_median_forgets_peaks_outside_its_window() {
        let settings = FluxSettings {
            average: 0.1,
            ..adaptive_settings()
        };
        let mut flux = pulse_train(0.3, 4);
        flux.extend(std::iter::repeat_n(0.0, 32));
        flux.extend(pulse_train(0.02, 1));
        let onsets = feed(&mut FluxTracker::default(), &settings, &flux);
        assert_eq!(
            onsets.len(),
            5,
            "the quiet hit after the gap must confirm against the absolute floor"
        );
    }

    #[test]
    fn the_onset_status_reports_the_peak_median_reference() {
        let settings = adaptive_settings();
        let mut tracker = FluxTracker::default();
        feed(&mut tracker, &settings, &pulse_train(0.1, 4));
        let status = onset_flux_status(&tracker, OnsetProjection::default());
        let reference = status.reference.expect("the peaks filled the median");
        assert!((reference - 0.1).abs() < 0.01);
    }

    #[test]
    fn the_flux_level_reference_averages_the_smoothed_flow() {
        let settings = FluxSettings {
            average: 0.1,
            sensitivity: 2.0,
            min_threshold: 0.0,
            ..adaptive_settings()
        };
        let mut tracker = FluxTracker::level();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..100 {
            now += dt;
            tracker.observe(0.5, now, dt, &settings);
        }
        assert!((tracker.threshold - 1.0).abs() < 1e-3);
        assert!(matches!(tracker.reference, OnsetReference::FluxLevel(_)));
    }

    #[test]
    fn the_default_kernels_select_their_bands() {
        let settings = defaults();
        assert!(settings.kick_contains(0));
        assert!(settings.kick_contains(15));
        assert!(settings.snare_contains(4));
        assert!(settings.snare_contains(15));
        assert!(!settings.snare_contains(1));
        assert!(settings.hat_contains(15));
        assert!(!settings.hat_contains(0));
    }

    #[test]
    fn the_hat_kernel_matches_high_band_flow() {
        let settings = defaults();
        let mut high = VecDeque::new();
        let mut low = VecDeque::new();
        for _ in 0..8 {
            high.push_back(std::array::from_fn(
                |band| if band >= 9 { 1.0 } else { 0.0 },
            ));
            low.push_back(std::array::from_fn(|band| if band < 6 { 1.0 } else { 0.0 }));
        }
        let high = settings.hat_pattern.project(
            &high,
            settings.hat_onset_suppression,
            settings.hat_onset_pre_gain,
            8,
        );
        let low = settings.hat_pattern.project(
            &low,
            settings.hat_onset_suppression,
            settings.hat_onset_pre_gain,
            8,
        );
        assert!(high.residual > low.residual);
        assert!(high.residual > 0.0);
    }

    #[test]
    fn a_kick_kernel_onset_fires_after_the_flux_peak() {
        let settings = defaults();
        let flux = settings.kick_onset_tracker();
        let mut kick = KickTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..8 {
            now += dt;
            kick.observe(0.0, now, dt, &flux);
        }
        now += dt;
        assert_eq!(kick.observe(1.0, now, dt, &flux), None);
        now += dt;
        let power = kick.observe(0.0, now, dt, &flux).expect("kick onset");
        assert!(power > 0.0);
        assert!(kick.flash > 0.0);
    }

    #[test]
    fn a_snare_onset_chokes_the_simultaneous_kick() {
        let mut settings = defaults();
        settings.snare_choke = 0.5;
        let dt = 1.0 / 86.0;
        let mut bank = OnsetBank::new();
        let quiet = Spectrum {
            bands: [0.5; BAND_COUNT],
            rms: 0.2,
            ..Spectrum::default()
        };
        let abdr = [1.0; BAND_COUNT];
        let abl = [1.0; BAND_COUNT];
        for _ in 0..8 {
            bank.observe(&quiet, &abdr, &abl, &settings, dt);
        }
        let pulse = Spectrum {
            bands: [0.5; BAND_COUNT],
            rms: 0.2,
            band_excess: [1.0; BAND_COUNT],
            ..Spectrum::default()
        };
        for _ in 0..4 {
            let events = bank.observe(&pulse, &abdr, &abl, &settings, dt);
            assert!(events.snare.is_none());
        }
        let events = bank.observe(&quiet, &abdr, &abl, &settings, dt);
        assert!(events.snare.is_some(), "snare onset");
        assert!(events.kick.is_none(), "the snare chokes the kick");
        assert!(bank.status().kick.choked);
    }

    fn snare_fires_with_rms(rms: f32, gate_db: f32) -> bool {
        let mut settings = defaults();
        settings.snare_min_rms_db = gate_db;
        let dt = 1.0 / 86.0;
        let mut bank = OnsetBank::new();
        let quiet = Spectrum {
            bands: [0.5; BAND_COUNT],
            rms,
            ..Spectrum::default()
        };
        let abdr = [1.0; BAND_COUNT];
        let abl = [1.0; BAND_COUNT];
        for _ in 0..8 {
            bank.observe(&quiet, &abdr, &abl, &settings, dt);
        }
        let pulse = Spectrum {
            bands: [0.5; BAND_COUNT],
            rms,
            band_excess: [1.0; BAND_COUNT],
            ..Spectrum::default()
        };
        for _ in 0..4 {
            bank.observe(&pulse, &abdr, &abl, &settings, dt);
        }
        bank.observe(&quiet, &abdr, &abl, &settings, dt)
            .snare
            .is_some()
    }

    #[test]
    fn the_snare_output_needs_the_absolute_rms_gate() {
        assert!(
            snare_fires_with_rms(0.2, -20.0),
            "a loud snare clears the gate"
        );
        assert!(
            !snare_fires_with_rms(0.01, -20.0),
            "a quiet snare must not reach the output"
        );
        assert!(
            snare_fires_with_rms(0.01, -60.0),
            "the permissive gate still accepts the quiet snare"
        );
    }

    #[test]
    fn the_suppression_kernel_is_independent_of_the_main_kernel() {
        let settings = defaults();
        let low_band = 0.5 / BAND_COUNT as f32;
        let high_band = 12.5 / BAND_COUNT as f32;
        assert!(
            settings
                .kick_pattern
                .suppression_field_value(0.05, low_band)
                > 0.0,
            "the kick suppressor covers the low bands"
        );
        assert_eq!(
            settings
                .kick_pattern
                .suppression_field_value(0.05, high_band),
            0.0,
            "the suppressor ignores the high bands"
        );
        assert!(
            settings.kick_pattern.field_value(0.05, high_band) > 0.0,
            "the main kernel still covers the high bands"
        );
    }

    #[test]
    fn band_onset_flow_applies_drp_and_abl() {
        let settings = defaults();
        let dt = 1.0 / 86.0;
        let mut bank = OnsetBank::new();
        let spectrum = Spectrum {
            band_excess: [0.5; BAND_COUNT],
            ..Spectrum::default()
        };
        let abdr = std::array::from_fn(|band| if band == 0 { 16.0 } else { 0.0 });
        let mut abl = [1.0; BAND_COUNT];
        abl[0] = 0.5;
        bank.observe(&spectrum, &abdr, &abl, &settings, dt);
        let status = bank.status();
        assert!((status.bands[0].drp - 16.0).abs() < 1e-6);
        assert!((status.bands[0].abl - 0.5).abs() < 1e-6);
        assert!((status.bands[0].onset - 4.0).abs() < 1e-6);
        assert_eq!(status.bands[1].onset, 0.0);
        bank.observe(&spectrum, &abdr, &abl, &settings, dt);
        assert!(
            (bank.status().bands[0].onset - 4.0).abs() < 1e-6,
            "the onset flow stays continuous while the excess is present"
        );
    }

    #[test]
    fn temporal_pattern_interpolates_between_polygons() {
        let pattern = TemporalPattern {
            inner: rectangle(0.25, 0.75, 0.25, 0.75),
            outer: rectangle(0.1, 0.9, 0.1, 0.9),
            suppression_inner: Vec::new(),
            suppression_outer: Vec::new(),
            effective: 1.0,
        };
        assert!((pattern.field_value(0.5, 0.5) - 1.0).abs() < 1e-6);
        assert_eq!(pattern.field_value(0.05, 0.5), 0.0);
        let edge = pattern.field_value(0.15, 0.5);
        assert!(edge > 0.0 && edge < 1.0, "annulus value was {edge}");
    }

    #[test]
    fn initial_patterns_stay_inside_the_normalized_grid() {
        for pattern in [
            TemporalPattern::kick_initial(),
            TemporalPattern::snare_initial(),
            TemporalPattern::hat_initial(),
        ] {
            pattern.validate().expect("initial patterns stay valid");
        }
    }

    #[test]
    fn effective_area_keeps_the_leftmost_part_and_fades_its_end() {
        let pattern = TemporalPattern {
            inner: rectangle(0.0, 1.0, 0.0, 1.0),
            outer: rectangle(0.0, 1.0, 0.0, 1.0),
            suppression_inner: Vec::new(),
            suppression_outer: Vec::new(),
            effective: 0.5,
        };
        assert!((pattern.effective_field_value(0.25, 0.5) - 1.0).abs() < 1e-6);
        assert_eq!(pattern.effective_field_value(0.5, 0.5), 0.0);
        assert_eq!(pattern.effective_field_value(0.9, 0.5), 0.0);
        assert!((pattern.effective_field_value(0.45, 0.5) - 1.0).abs() < 1e-6);
        assert!((pattern.effective_field_value(0.48, 0.5) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn onset_pre_gain_trims_the_pattern_projection() {
        let mut settings = defaults();
        settings.kick_onset_suppression = 0.0;
        settings.kick_onset_pre_gain = 0.25;
        let history = VecDeque::from([[1.0; BAND_COUNT]]);
        let full = settings
            .kick_pattern
            .project(&history, settings.kick_onset_suppression, 1.0, 1);
        let projection = settings.kick_pattern.project(
            &history,
            settings.kick_onset_suppression,
            settings.kick_onset_pre_gain,
            1,
        );
        assert!((projection.residual - full.residual * 0.25).abs() < 1e-6);
        assert!((projection.inhibition - full.inhibition * 0.25).abs() < 1e-6);
        assert!((projection.suppressed - 0.0).abs() < 1e-6);
    }

    #[test]
    fn the_suppression_kernel_suppresses_other_instrument_flow() {
        let mut settings = defaults();
        settings.snare_onset_suppression = 1.0;
        let mut low = [0.0; BAND_COUNT];
        low[..5].fill(1.0);
        let low_history = VecDeque::from([low]);
        let projection = settings.snare_pattern.project(
            &low_history,
            settings.snare_onset_suppression,
            settings.snare_onset_pre_gain,
            1,
        );
        assert_eq!(
            projection.residual, 0.0,
            "a low-only transient must not confirm a snare"
        );
        assert!(projection.inhibition > 0.0);
        assert_eq!(projection.suppressed, projection.inhibition);

        let broadband = VecDeque::from([[1.0; BAND_COUNT]]);
        let unsuppressed =
            settings
                .snare_pattern
                .project(&broadband, 0.0, settings.snare_onset_pre_gain, 1);
        settings.snare_onset_suppression = 0.5;
        let projection = settings.snare_pattern.project(
            &broadband,
            settings.snare_onset_suppression,
            settings.snare_onset_pre_gain,
            1,
        );
        assert!((projection.inhibition - unsuppressed.inhibition).abs() < 1e-6);
        assert!(
            (projection.suppressed - projection.inhibition * 0.5).abs() < 1e-6,
            "half the suppression projection is subtracted from a broadband hit"
        );
        assert!(
            (projection.residual - (unsuppressed.residual - projection.suppressed).max(0.0)).abs()
                < 1e-6
        );
    }

    fn gap_settings() -> DetectorSettings {
        DetectorSettings {
            drop_kernels: DropKernels {
                onset: [1.0; BAND_COUNT],
            },
            drop_gap_threshold: -18.0,
            ..DetectorSettings::default()
        }
    }

    #[test]
    fn the_rms_envelope_arms_the_gap_below_the_threshold() {
        let settings = gap_settings();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut drop = DropTracker::default();
        for _ in 0..12 {
            now += dt;
            drop.observe(now, 0.5, 0.0, &settings, dt);
        }
        assert!(!drop.eligible);
        assert!(drop.envelope_db > settings.drop_gap_threshold);

        let mut entries = 0;
        for _ in 0..60 {
            now += dt;
            drop.observe(now, 0.01, 0.0, &settings, dt);
            entries += u8::from(drop.gap_entry);
        }
        assert!(drop.eligible, "the quiet RMS arms the gate");
        assert!(drop.envelope_db < settings.drop_gap_threshold);
        assert_eq!(entries, 1, "the latch reports one entry edge");
    }

    #[test]
    fn the_gap_gate_threshold_is_configurable() {
        let dt = 1.0 / 86.0;
        for (threshold, expected) in [(-30.0, false), (-20.0, true)] {
            let settings = DetectorSettings {
                drop_gap_threshold: threshold,
                ..gap_settings()
            };
            let mut drop = DropTracker::default();
            let mut now = 0.0;
            for _ in 0..60 {
                now += dt;
                drop.observe(now, 0.05, 0.0, &settings, dt);
            }
            assert_eq!(drop.eligible, expected, "gate {threshold} dB");
        }
    }

    #[test]
    fn the_gate_latches_until_an_onset_clears_it() {
        let settings = gap_settings();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut drop = DropTracker::default();
        for _ in 0..60 {
            now += dt;
            drop.observe(now, 0.01, 0.0, &settings, dt);
        }
        assert!(drop.eligible);

        for _ in 0..30 {
            now += dt;
            drop.observe(now, 0.8, 0.0, &settings, dt);
        }
        assert!(drop.envelope_db > settings.drop_gap_threshold);
        assert!(drop.eligible, "the gate stays latched while loud");

        now += dt;
        drop.observe(now, 0.8, 1.0, &settings, dt);
        now += dt;
        assert!(drop.observe(now, 0.8, 0.0, &settings, dt).is_some());
        assert!(!drop.eligible, "the confirming onset clears the latch");
    }

    #[test]
    fn drop_confirmation_requires_an_armed_gate() {
        let settings = gap_settings();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut drop = DropTracker::default();
        for _ in 0..12 {
            now += dt;
            drop.observe(now, 0.8, 0.0, &settings, dt);
        }
        assert!(!drop.eligible);
        now += dt;
        drop.observe(now, 0.8, 1.0, &settings, dt);
        now += dt;
        assert_eq!(
            drop.observe(now, 0.8, 0.0, &settings, dt),
            None,
            "an onset without a quiet RMS is not a drop"
        );
    }

    #[test]
    fn the_composite_onset_uses_its_own_flux_settings() {
        let mut settings = gap_settings();
        settings.drop_onset_min_threshold = 2.0;
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut drop = DropTracker::default();
        for _ in 0..60 {
            now += dt;
            drop.observe(now, 0.01, 0.0, &settings, dt);
        }
        assert!(drop.eligible);
        now += dt;
        drop.observe(now, 0.01, 1.0, &settings, dt);
        now += dt;
        assert_eq!(
            drop.observe(now, 0.01, 0.0, &settings, dt),
            None,
            "the dedicated minimum threshold blocks the composite onset"
        );

        settings.drop_onset_min_threshold = 0.01;
        for _ in 0..40 {
            now += dt;
            drop.observe(now, 0.01, 0.0, &settings, dt);
        }
        now += dt;
        drop.observe(now, 0.01, 1.0, &settings, dt);
        now += dt;
        assert!(drop.observe(now, 0.01, 0.0, &settings, dt).is_some());
    }

    #[test]
    fn resetting_clears_the_rms_envelope_and_gate() {
        let settings = gap_settings();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let mut drop = DropTracker::default();
        for _ in 0..60 {
            now += dt;
            drop.observe(now, 0.01, 0.0, &settings, dt);
        }
        assert!(drop.eligible);
        drop.reset();
        assert!(!drop.eligible);
        assert!(!drop.envelope_ready);
        assert_eq!(drop.envelope_db, GAP_DB_FLOOR);
        assert_eq!(drop.rms_db, GAP_DB_FLOOR);
    }

    #[test]
    fn drop_kernel_weights_stay_inside_zero_to_one() {
        let mut kernels = DropKernels::default();
        kernels.set(DropKernel::Onset, 2, 2.0);
        assert_eq!(kernels.onset[2], 1.0);
        kernels.set(DropKernel::Onset, 3, -1.0);
        assert_eq!(kernels.onset[3], 0.0);
        assert_eq!(kernels.weights(DropKernel::Onset), kernels.onset);
    }

    #[test]
    fn the_quake_sequence_advances_every_nth_kick_and_resets() {
        let mut sequence = QuakeSequence::default();
        assert_eq!(sequence.next(1), QuakeDirection::Left);
        assert_eq!(sequence.next(1), QuakeDirection::Top);
        assert_eq!(sequence.next(1), QuakeDirection::Right);
        assert_eq!(sequence.next(1), QuakeDirection::Bottom);
        sequence.reset();
        assert_eq!(sequence.next(2), QuakeDirection::Left);
        assert_eq!(sequence.next(2), QuakeDirection::Left);
        assert_eq!(sequence.next(2), QuakeDirection::Top);
        assert_eq!(QuakeDirection::Top.unit(), (0.0, -1.0));
    }

    #[test]
    fn a_damped_spring_overshoots_and_settles_on_its_target() {
        let mut spring = Spring::default();
        spring.set_target(1.0);
        let mut peak: f32 = 0.0;
        for _ in 0..240 {
            spring.advance(1.0 / 120.0, 70.0, 7.0);
            peak = peak.max(spring.value());
        }
        assert!(peak > 1.0, "an underdamped spring overshoots");
        assert!((spring.value() - 1.0).abs() < 0.01);
    }

    #[test]
    fn invalid_settings_are_rejected_early() {
        let mut settings = defaults();
        settings.band_onset[0].abl_gain = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.excess.cap = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.kick_onset_pre_gain = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.kick_pattern.inner[0][0] = -0.1;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.snare_pattern.outer[0][1] = f32::NAN;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.kick_onset_attack = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.snare_onset_sensitivity = 9.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.snare_min_rms_db = 1.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.snare_min_rms_db = -61.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.snare_min_rms_db = f32::NAN;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_threshold = 1.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_threshold = -61.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_threshold = f32::NAN;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_kernels.onset[0] = -0.1;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_kernels.onset[0] = f32::NAN;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_onset_attack = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_onset_suppression = 3.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.kick_sequence_every = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn a_flux_pulse_reports_a_positive_onset_power() {
        let settings = FluxSettings::default();
        let mut tracker = FluxTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for flux in [0.0, 0.0, 1.0, 0.0] {
            now += dt;
            if let Some(power) = tracker.observe(flux, now, dt, &settings) {
                assert!(power > 0.0);
                return;
            }
        }
        panic!("expected a flux onset");
    }
}
