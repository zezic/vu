//! Per-band onset tracking and the kick, snare, and drop detectors.
//!
//! Every band runs a spectral-flux detector: positive log-magnitude changes
//! are summed over the band's FFT bins, smoothed with an attack/release filter,
//! and compared with a slow adaptive average. A local peak above that threshold
//! becomes a band onset.
//!
//! The drop detector projects the band levels onto a low and a high kernel,
//! accumulates their difference in a rolling window, and gates a composite
//! onset detector with that window's confidence.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::spectrum::Spectrum;
use super::BAND_COUNT;

pub const FLUX_ATTACK_MIN: f32 = 0.001;
pub const FLUX_ATTACK_MAX: f32 = 0.1;
pub const FLUX_RELEASE_MIN: f32 = 0.005;
pub const FLUX_RELEASE_MAX: f32 = 0.5;
pub const FLUX_AVERAGE_MIN: f32 = 0.05;
pub const FLUX_AVERAGE_MAX: f32 = 1.0;
pub const FLUX_SENSITIVITY_MIN: f32 = 0.5;
pub const FLUX_SENSITIVITY_MAX: f32 = 5.0;
pub const FLUX_THRESHOLD_MIN: f32 = 0.0;
pub const FLUX_THRESHOLD_MAX: f32 = 2.0;
/// Upper bound of the dedicated composite-onset minimum threshold.
pub const ONSET_THRESHOLD_MAX: f32 = 0.25;
pub const FLUX_INTERVAL_MIN: f32 = 0.005;
pub const FLUX_INTERVAL_MAX: f32 = 0.2;
pub const CHOKE_MIN: f32 = 0.0;
pub const CHOKE_MAX: f32 = 2.0;
pub const QUAKE_FACTOR_MIN: f32 = 0.0;
pub const QUAKE_FACTOR_MAX: f32 = 20.0;
pub const SEQUENCE_EVERY_MIN: u32 = 1;
pub const SEQUENCE_EVERY_MAX: u32 = 32;
pub const DROP_GAP_WINDOW_MIN: f32 = 0.1;
pub const DROP_GAP_WINDOW_MAX: f32 = 4.0;
pub const DROP_GAP_THRESHOLD_MIN: f32 = -1.0;
pub const DROP_GAP_THRESHOLD_MAX: f32 = 1.0;
pub const DROP_GAP_GAIN_MIN: f32 = 0.1;
pub const DROP_GAP_GAIN_MAX: f32 = 10.0;
pub const DROP_KERNEL_MIN: f32 = 0.0;
pub const DROP_KERNEL_MAX: f32 = 1.0;
pub const SPRING_STIFFNESS_MIN: f32 = 1.0;
pub const SPRING_STIFFNESS_MAX: f32 = 400.0;
pub const SPRING_DAMPING_MIN: f32 = 0.0;
pub const SPRING_DAMPING_MAX: f32 = 60.0;

const FLASH_DECAY: f32 = 8.0;
const SPRING_STEP: f32 = 1.0 / 240.0;
/// Worst-case gap window samples: four seconds at a 2.7 ms hop.
const GAP_WINDOW_CAPACITY: usize = 2048;
const KERNEL_EPSILON: f32 = 1e-6;

/// One of the three drawable drop projections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropKernel {
    Low,
    High,
    Onset,
}

impl DropKernel {
    pub const ALL: [Self; 3] = [Self::Low, Self::High, Self::Onset];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::High => "HIGH",
            Self::Onset => "ONSET",
        }
    }
}

/// One drawable kernel in the detector inspector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectorKernel {
    Kick,
    Snare,
    Drop(DropKernel),
}

impl DetectorKernel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Kick => "KICK",
            Self::Snare => "SNARE",
            Self::Drop(kernel) => kernel.label(),
        }
    }
}

/// Per-band weights for the drop detector's low, high, and onset projections.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropKernels {
    pub low: [f32; BAND_COUNT],
    pub high: [f32; BAND_COUNT],
    pub onset: [f32; BAND_COUNT],
}

impl Default for DropKernels {
    fn default() -> Self {
        Self {
            low: std::array::from_fn(|band| if band < 4 { 1.0 } else { 0.0 }),
            high: std::array::from_fn(|band| if band >= 4 { 1.0 } else { 0.0 }),
            onset: std::array::from_fn(|band| if band < 4 { 1.0 } else { 0.0 }),
        }
    }
}

impl DropKernels {
    pub fn weights(self, kernel: DropKernel) -> [f32; BAND_COUNT] {
        match kernel {
            DropKernel::Low => self.low,
            DropKernel::High => self.high,
            DropKernel::Onset => self.onset,
        }
    }

    /// Weighted mean of the values. A zero-area kernel projects to zero.
    pub fn projection(self, kernel: DropKernel, values: &[f32; BAND_COUNT]) -> f32 {
        kernel_projection(&self.weights(kernel), values)
    }

    pub fn set(&mut self, kernel: DropKernel, band: usize, value: f32) {
        assert!(
            band < BAND_COUNT,
            "drop kernel bands stay inside the spectrum"
        );
        let value = value.clamp(DROP_KERNEL_MIN, DROP_KERNEL_MAX);
        match kernel {
            DropKernel::Low => self.low[band] = value,
            DropKernel::High => self.high[band] = value,
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

/// All user-tunable detector configuration. The whole value persists in the
/// config file and is mirrored by the inspector sliders.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "DetectorSettingsRaw", into = "DetectorSettingsRaw")]
pub struct DetectorSettings {
    /// Flux smoothing attack and release time constants in seconds.
    pub flux_attack: f32,
    pub flux_release: f32,
    /// Slow exponential average that forms the adaptive threshold.
    pub flux_average: f32,
    pub flux_sensitivity: f32,
    pub flux_min_threshold: f32,
    /// Do not report another onset before this interval has elapsed.
    pub flux_min_interval: f32,
    /// Per-band weights for the kick onset projection.
    pub kick_kernel: [f32; BAND_COUNT],
    /// Dedicated flux tracker settings for the kick onset kernel.
    pub kick_onset_attack: f32,
    pub kick_onset_release: f32,
    pub kick_onset_average: f32,
    pub kick_onset_sensitivity: f32,
    pub kick_onset_min_threshold: f32,
    pub kick_onset_min_interval: f32,
    /// Time the kick stays muted after a confirmed snare or drop.
    pub kick_snare_choke: f32,
    pub kick_drop_choke: f32,
    pub kick_quake_factor: f32,
    /// A new quake direction is taken every this many kicks.
    pub kick_sequence_every: u32,
    /// Per-band weights for the snare onset projection.
    pub snare_kernel: [f32; BAND_COUNT],
    /// Dedicated flux tracker settings for the snare onset kernel.
    pub snare_onset_attack: f32,
    pub snare_onset_release: f32,
    pub snare_onset_average: f32,
    pub snare_onset_sensitivity: f32,
    pub snare_onset_min_threshold: f32,
    pub snare_onset_min_interval: f32,
    pub snare_flip_every: u32,
    pub snare_choke: f32,
    pub flip_stiffness: f32,
    pub flip_damping: f32,
    /// Per-band weights for the low, high, and onset projections.
    pub drop_kernels: DropKernels,
    /// Length of the rolling high-minus-low window in seconds.
    pub drop_gap_window: f32,
    /// Scaled mean high-minus-low difference required before an onset can
    /// confirm.
    pub drop_gap_threshold: f32,
    /// Multiplies the window mean around zero before the threshold comparison.
    pub drop_gap_gain: f32,
    /// Dedicated flux tracker settings for the composite onset kernel.
    pub drop_onset_attack: f32,
    pub drop_onset_release: f32,
    pub drop_onset_average: f32,
    pub drop_onset_sensitivity: f32,
    pub drop_onset_min_threshold: f32,
    pub drop_onset_min_interval: f32,
    pub drop_quake_factor: f32,
    pub quake_stiffness: f32,
    pub quake_damping: f32,
}

impl Default for DetectorSettings {
    fn default() -> Self {
        Self {
            flux_attack: 0.005,
            flux_release: 0.05,
            flux_average: 0.2,
            flux_sensitivity: 1.5,
            flux_min_threshold: 0.01,
            flux_min_interval: 0.03,
            kick_kernel: std::array::from_fn(|band| if band < 5 { 1.0 } else { 0.0 }),
            kick_onset_attack: 0.005,
            kick_onset_release: 0.05,
            kick_onset_average: 0.2,
            kick_onset_sensitivity: 1.5,
            kick_onset_min_threshold: 0.01,
            kick_onset_min_interval: 0.03,
            kick_snare_choke: 0.12,
            kick_drop_choke: 0.2,
            kick_quake_factor: 2.0,
            kick_sequence_every: 1,
            snare_kernel: std::array::from_fn(|band| {
                if matches!(band, 5 | 6 | 10 | 12) {
                    1.0
                } else {
                    0.0
                }
            }),
            snare_onset_attack: 0.005,
            snare_onset_release: 0.05,
            snare_onset_average: 0.2,
            snare_onset_sensitivity: 1.5,
            snare_onset_min_threshold: 0.01,
            snare_onset_min_interval: 0.03,
            snare_flip_every: 4,
            snare_choke: 0.12,
            flip_stiffness: 70.0,
            flip_damping: 7.0,
            drop_kernels: DropKernels::default(),
            drop_gap_window: 1.0,
            drop_gap_threshold: 0.15,
            drop_gap_gain: 1.0,
            drop_onset_attack: 0.005,
            drop_onset_release: 0.05,
            drop_onset_average: 0.2,
            drop_onset_sensitivity: 1.5,
            drop_onset_min_threshold: 0.01,
            drop_onset_min_interval: 0.03,
            drop_quake_factor: 1.0,
            quake_stiffness: 140.0,
            quake_damping: 9.0,
        }
    }
}

impl DetectorSettings {
    pub fn validate(self) -> anyhow::Result<Self> {
        for (name, value, minimum, maximum) in [
            (
                "flux_attack",
                self.flux_attack,
                FLUX_ATTACK_MIN,
                FLUX_ATTACK_MAX,
            ),
            (
                "flux_release",
                self.flux_release,
                FLUX_RELEASE_MIN,
                FLUX_RELEASE_MAX,
            ),
            (
                "flux_average",
                self.flux_average,
                FLUX_AVERAGE_MIN,
                FLUX_AVERAGE_MAX,
            ),
            (
                "flux_sensitivity",
                self.flux_sensitivity,
                FLUX_SENSITIVITY_MIN,
                FLUX_SENSITIVITY_MAX,
            ),
            (
                "flux_min_threshold",
                self.flux_min_threshold,
                FLUX_THRESHOLD_MIN,
                FLUX_THRESHOLD_MAX,
            ),
            (
                "flux_min_interval",
                self.flux_min_interval,
                FLUX_INTERVAL_MIN,
                FLUX_INTERVAL_MAX,
            ),
        ] {
            anyhow::ensure!(
                value.is_finite() && (minimum..=maximum).contains(&value),
                "{name} must be finite and between {minimum} and {maximum}"
            );
        }
        for (name, value, minimum, maximum) in [
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
        ] {
            anyhow::ensure!(
                value.is_finite() && (minimum..=maximum).contains(&value),
                "{name} must be finite and between {minimum} and {maximum}"
            );
        }
        for (name, value, minimum, maximum) in [
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
        ] {
            anyhow::ensure!(
                value.is_finite() && (minimum..=maximum).contains(&value),
                "{name} must be finite and between {minimum} and {maximum}"
            );
        }
        validate_kernel("kick", &self.kick_kernel)?;
        validate_kernel("snare", &self.snare_kernel)?;
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
        self.drop_kernels.validate()?;
        anyhow::ensure!(
            self.drop_gap_window.is_finite()
                && (DROP_GAP_WINDOW_MIN..=DROP_GAP_WINDOW_MAX).contains(&self.drop_gap_window),
            "the drop gap window must be finite and between \
             {DROP_GAP_WINDOW_MIN} and {DROP_GAP_WINDOW_MAX}"
        );
        anyhow::ensure!(
            self.drop_gap_threshold.is_finite()
                && (DROP_GAP_THRESHOLD_MIN..=DROP_GAP_THRESHOLD_MAX)
                    .contains(&self.drop_gap_threshold),
            "the drop gap threshold must be finite and between \
             {DROP_GAP_THRESHOLD_MIN} and {DROP_GAP_THRESHOLD_MAX}"
        );
        anyhow::ensure!(
            self.drop_gap_gain.is_finite()
                && (DROP_GAP_GAIN_MIN..=DROP_GAP_GAIN_MAX).contains(&self.drop_gap_gain),
            "the drop gap gain must be finite and between \
             {DROP_GAP_GAIN_MIN} and {DROP_GAP_GAIN_MAX}"
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

    fn flux_settings(&self) -> FluxSettings {
        FluxSettings {
            attack: self.flux_attack,
            release: self.flux_release,
            average: self.flux_average,
            sensitivity: self.flux_sensitivity,
            min_threshold: self.flux_min_threshold,
            min_interval: self.flux_min_interval,
        }
    }

    fn drop_onset_flux_settings(&self) -> FluxSettings {
        FluxSettings {
            attack: self.drop_onset_attack,
            release: self.drop_onset_release,
            average: self.drop_onset_average,
            sensitivity: self.drop_onset_sensitivity,
            min_threshold: self.drop_onset_min_threshold,
            min_interval: self.drop_onset_min_interval,
        }
    }

    fn kick_onset_flux_settings(&self) -> FluxSettings {
        FluxSettings {
            attack: self.kick_onset_attack,
            release: self.kick_onset_release,
            average: self.kick_onset_average,
            sensitivity: self.kick_onset_sensitivity,
            min_threshold: self.kick_onset_min_threshold,
            min_interval: self.kick_onset_min_interval,
        }
    }

    fn snare_onset_flux_settings(&self) -> FluxSettings {
        FluxSettings {
            attack: self.snare_onset_attack,
            release: self.snare_onset_release,
            average: self.snare_onset_average,
            sensitivity: self.snare_onset_sensitivity,
            min_threshold: self.snare_onset_min_threshold,
            min_interval: self.snare_onset_min_interval,
        }
    }

    pub fn kick_contains(&self, band: usize) -> bool {
        self.kick_kernel[band] > 0.0
    }

    pub fn snare_contains(&self, band: usize) -> bool {
        self.snare_kernel[band] > 0.0
    }

    pub fn kernel_weights(&self, kernel: DetectorKernel) -> [f32; BAND_COUNT] {
        match kernel {
            DetectorKernel::Kick => self.kick_kernel,
            DetectorKernel::Snare => self.snare_kernel,
            DetectorKernel::Drop(kernel) => self.drop_kernels.weights(kernel),
        }
    }

    pub fn set_kernel_band(&mut self, kernel: DetectorKernel, band: usize, value: f32) {
        assert!(band < BAND_COUNT, "kernel bands stay inside the spectrum");
        let value = value.clamp(DROP_KERNEL_MIN, DROP_KERNEL_MAX);
        match kernel {
            DetectorKernel::Kick => self.kick_kernel[band] = value,
            DetectorKernel::Snare => self.snare_kernel[band] = value,
            DetectorKernel::Drop(kernel) => self.drop_kernels.set(kernel, band, value),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DetectorSettingsRaw {
    flux_attack: f32,
    flux_release: f32,
    flux_average: f32,
    flux_sensitivity: f32,
    flux_min_threshold: f32,
    flux_min_interval: f32,
    kick_kernel: [f32; BAND_COUNT],
    kick_onset_attack: f32,
    kick_onset_release: f32,
    kick_onset_average: f32,
    kick_onset_sensitivity: f32,
    kick_onset_min_threshold: f32,
    kick_onset_min_interval: f32,
    kick_snare_choke: f32,
    kick_drop_choke: f32,
    kick_quake_factor: f32,
    kick_sequence_every: u32,
    snare_kernel: [f32; BAND_COUNT],
    snare_onset_attack: f32,
    snare_onset_release: f32,
    snare_onset_average: f32,
    snare_onset_sensitivity: f32,
    snare_onset_min_threshold: f32,
    snare_onset_min_interval: f32,
    snare_flip_every: u32,
    snare_choke: f32,
    flip_stiffness: f32,
    flip_damping: f32,
    drop_gap_window: f32,
    drop_gap_threshold: f32,
    drop_gap_gain: f32,
    drop_onset_attack: f32,
    drop_onset_release: f32,
    drop_onset_average: f32,
    drop_onset_sensitivity: f32,
    drop_onset_min_threshold: f32,
    drop_onset_min_interval: f32,
    drop_low_kernel: [f32; BAND_COUNT],
    drop_high_kernel: [f32; BAND_COUNT],
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
        DetectorSettings::default().into()
    }
}

impl TryFrom<DetectorSettingsRaw> for DetectorSettings {
    type Error = anyhow::Error;

    fn try_from(raw: DetectorSettingsRaw) -> Result<Self, Self::Error> {
        Self {
            flux_attack: raw.flux_attack,
            flux_release: raw.flux_release,
            flux_average: raw.flux_average,
            flux_sensitivity: raw.flux_sensitivity,
            flux_min_threshold: raw.flux_min_threshold,
            flux_min_interval: raw.flux_min_interval,
            kick_kernel: raw.kick_kernel,
            kick_onset_attack: raw.kick_onset_attack,
            kick_onset_release: raw.kick_onset_release,
            kick_onset_average: raw.kick_onset_average,
            kick_onset_sensitivity: raw.kick_onset_sensitivity,
            kick_onset_min_threshold: raw.kick_onset_min_threshold,
            kick_onset_min_interval: raw.kick_onset_min_interval,
            kick_snare_choke: raw.kick_snare_choke,
            kick_drop_choke: raw.kick_drop_choke,
            kick_quake_factor: raw.kick_quake_factor,
            kick_sequence_every: raw.kick_sequence_every,
            snare_kernel: raw.snare_kernel,
            snare_onset_attack: raw.snare_onset_attack,
            snare_onset_release: raw.snare_onset_release,
            snare_onset_average: raw.snare_onset_average,
            snare_onset_sensitivity: raw.snare_onset_sensitivity,
            snare_onset_min_threshold: raw.snare_onset_min_threshold,
            snare_onset_min_interval: raw.snare_onset_min_interval,
            snare_flip_every: raw.snare_flip_every,
            snare_choke: raw.snare_choke,
            flip_stiffness: raw.flip_stiffness,
            flip_damping: raw.flip_damping,
            drop_kernels: DropKernels {
                low: raw.drop_low_kernel,
                high: raw.drop_high_kernel,
                onset: raw.drop_onset_kernel,
            },
            drop_gap_window: raw.drop_gap_window,
            drop_gap_threshold: raw.drop_gap_threshold,
            drop_gap_gain: raw.drop_gap_gain,
            drop_onset_attack: raw.drop_onset_attack,
            drop_onset_release: raw.drop_onset_release,
            drop_onset_average: raw.drop_onset_average,
            drop_onset_sensitivity: raw.drop_onset_sensitivity,
            drop_onset_min_threshold: raw.drop_onset_min_threshold,
            drop_onset_min_interval: raw.drop_onset_min_interval,
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
            flux_attack: settings.flux_attack,
            flux_release: settings.flux_release,
            flux_average: settings.flux_average,
            flux_sensitivity: settings.flux_sensitivity,
            flux_min_threshold: settings.flux_min_threshold,
            flux_min_interval: settings.flux_min_interval,
            kick_kernel: settings.kick_kernel,
            kick_onset_attack: settings.kick_onset_attack,
            kick_onset_release: settings.kick_onset_release,
            kick_onset_average: settings.kick_onset_average,
            kick_onset_sensitivity: settings.kick_onset_sensitivity,
            kick_onset_min_threshold: settings.kick_onset_min_threshold,
            kick_onset_min_interval: settings.kick_onset_min_interval,
            kick_snare_choke: settings.kick_snare_choke,
            kick_drop_choke: settings.kick_drop_choke,
            kick_quake_factor: settings.kick_quake_factor,
            kick_sequence_every: settings.kick_sequence_every,
            snare_kernel: settings.snare_kernel,
            snare_onset_attack: settings.snare_onset_attack,
            snare_onset_release: settings.snare_onset_release,
            snare_onset_average: settings.snare_onset_average,
            snare_onset_sensitivity: settings.snare_onset_sensitivity,
            snare_onset_min_threshold: settings.snare_onset_min_threshold,
            snare_onset_min_interval: settings.snare_onset_min_interval,
            snare_flip_every: settings.snare_flip_every,
            snare_choke: settings.snare_choke,
            flip_stiffness: settings.flip_stiffness,
            flip_damping: settings.flip_damping,
            drop_gap_window: settings.drop_gap_window,
            drop_gap_threshold: settings.drop_gap_threshold,
            drop_gap_gain: settings.drop_gap_gain,
            drop_onset_attack: settings.drop_onset_attack,
            drop_onset_release: settings.drop_onset_release,
            drop_onset_average: settings.drop_onset_average,
            drop_onset_sensitivity: settings.drop_onset_sensitivity,
            drop_onset_min_threshold: settings.drop_onset_min_threshold,
            drop_onset_min_interval: settings.drop_onset_min_interval,
            drop_low_kernel: settings.drop_kernels.low,
            drop_high_kernel: settings.drop_kernels.high,
            drop_onset_kernel: settings.drop_kernels.onset,
            drop_quake_factor: settings.drop_quake_factor,
            quake_stiffness: settings.quake_stiffness,
            quake_damping: settings.quake_damping,
            legacy_drop_split: 0,
            legacy_drop_onsets: 0,
            legacy_drop_window: 0.0,
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

/// One confirmed flux peak on a band. Power is the peak's ratio to its adaptive
/// threshold, so cumulative powers stay independent of the absolute FFT scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    pub band: usize,
    pub power: f32,
}

/// Onsets produced by one analysis hop, plus the confirmed detectors.
#[derive(Clone, Copy, Debug, Default)]
pub struct DetectorEvents {
    pub bands: [Option<Onset>; BAND_COUNT],
    pub kick: Option<f32>,
    pub snare: Option<f32>,
    pub drop: Option<f32>,
}

/// One spectral-flux tracker recipe. Per-band onsets and the drop's composite
/// onset each keep their own copy of these settings.
#[derive(Clone, Copy, Debug)]
struct FluxSettings {
    pub attack: f32,
    pub release: f32,
    pub average: f32,
    pub sensitivity: f32,
    pub min_threshold: f32,
    pub min_interval: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct FluxTracker {
    raw: f32,
    smoothed: f32,
    average: f32,
    previous_smoothed: f32,
    before_previous_smoothed: f32,
    threshold: f32,
    last_onset: f32,
    primed: bool,
    triggers: u32,
}

impl FluxTracker {
    fn observe(&mut self, flux: f32, now: f32, dt: f32, settings: &FluxSettings) -> Option<f32> {
        self.raw = flux;
        if !self.primed {
            self.smoothed = flux;
            self.average = flux;
            self.previous_smoothed = flux;
            self.before_previous_smoothed = flux;
            self.threshold = (self.average * settings.sensitivity).max(settings.min_threshold);
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
        let average_alpha = (-dt / settings.average).exp();
        self.average = average_alpha * self.average + (1.0 - average_alpha) * self.smoothed;
        let threshold = (self.average * settings.sensitivity).max(settings.min_threshold);

        let previous = self.previous_smoothed;
        let before_previous = self.before_previous_smoothed;
        let previous_threshold = self.threshold;
        let peak = previous >= before_previous
            && previous > self.smoothed
            && previous >= previous_threshold
            && now - dt - self.last_onset >= settings.min_interval;
        self.before_previous_smoothed = previous;
        self.previous_smoothed = self.smoothed;
        self.threshold = threshold;
        if peak {
            self.last_onset = now - dt;
            self.triggers = self.triggers.wrapping_add(1);
            // Strength over the adaptive threshold keeps the confirmed
            // detectors independent of the absolute FFT magnitude scale.
            Some(previous / previous_threshold.max(1e-6))
        } else {
            None
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
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
    fn observe(&mut self, flux: f32, now: f32, dt: f32, settings: &FluxSettings) -> Option<f32> {
        self.flash *= (-dt * FLASH_DECAY).exp();
        let power = self.onset.observe(flux, now, dt, settings)?;
        self.flash = 1.0;
        self.last_power = power;
        Some(power)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Rolling time window of high-minus-low differences with a running sum.
#[derive(Debug, Default)]
struct GapWindow {
    samples: VecDeque<(f32, f32)>,
    sum: f32,
    /// Time the window started filling after the last clear.
    started_at: Option<f32>,
}

impl GapWindow {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            sum: 0.0,
            started_at: None,
        }
    }

    fn push(&mut self, at: f32, value: f32, window: f32) {
        self.started_at.get_or_insert(at);
        self.samples.push_back((at, value));
        self.sum += value;
        self.prune(at, window);
    }

    fn prune(&mut self, now: f32, window: f32) {
        while let Some(&(at, value)) = self.samples.front() {
            if now - at <= window {
                break;
            }
            self.samples.pop_front();
            self.sum -= value;
        }
    }

    fn mean(&self) -> f32 {
        if self.samples.is_empty() {
            0.0
        } else {
            self.sum / self.samples.len() as f32
        }
    }

    /// A partial window cannot confirm a drop; it must cover the whole span.
    fn is_full(&self, now: f32, window: f32) -> bool {
        self.started_at
            .is_some_and(|started| now - started >= window)
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.sum = 0.0;
        self.started_at = None;
    }
}

#[derive(Debug)]
struct DropTracker {
    window: GapWindow,
    onset: FluxTracker,
    low: f32,
    high: f32,
    diff: f32,
    confidence: f32,
    /// Latches high when the window is confident; an onset resets it.
    eligible: bool,
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
            window: GapWindow::with_capacity(GAP_WINDOW_CAPACITY),
            onset: FluxTracker::default(),
            low: 0.0,
            high: 0.0,
            diff: 0.0,
            confidence: 0.0,
            eligible: false,
            flash: 0.0,
            last_power: 0.0,
        }
    }

    fn observe(
        &mut self,
        now: f32,
        spectrum: &Spectrum,
        settings: &DetectorSettings,
        dt: f32,
    ) -> Option<f32> {
        self.flash *= (-dt * FLASH_DECAY).exp();
        self.low = settings
            .drop_kernels
            .projection(DropKernel::Low, &spectrum.bands);
        self.high = settings
            .drop_kernels
            .projection(DropKernel::High, &spectrum.bands);
        self.diff = self.high - self.low;
        self.window.push(now, self.diff, settings.drop_gap_window);
        self.confidence = self.window.mean() * settings.drop_gap_gain;
        // A confident window latches eligibility high. Only a composite
        // onset clears it again.
        self.eligible |= self.window.is_full(now, settings.drop_gap_window)
            && self.confidence >= settings.drop_gap_threshold;

        let onset_flux = settings
            .drop_kernels
            .projection(DropKernel::Onset, &spectrum.band_flux);
        let flux_settings = settings.drop_onset_flux_settings();
        let power = self.onset.observe(onset_flux, now, dt, &flux_settings)?;
        if !self.eligible {
            return None;
        }
        self.window.clear();
        self.confidence = 0.0;
        self.eligible = false;
        self.flash = 1.0;
        self.last_power = power;
        Some(power)
    }

    fn reset(&mut self) {
        self.window.clear();
        self.onset.reset();
        self.low = 0.0;
        self.high = 0.0;
        self.diff = 0.0;
        self.confidence = 0.0;
        self.eligible = false;
        self.flash = 0.0;
        self.last_power = 0.0;
    }
}

/// All per-band trackers and the three confirmed detectors. One `observe` call
/// advances the state by one analysis hop.
pub struct OnsetBank {
    bands: [FluxTracker; BAND_COUNT],
    kick: KickTracker,
    snare: SnareTracker,
    drop: DropTracker,
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
            bands: [FluxTracker::default(); BAND_COUNT],
            kick: KickTracker::default(),
            snare: SnareTracker::default(),
            drop: DropTracker::default(),
            time: 0.0,
        }
    }

    /// Feeds one analysis hop. The kick, snare, and drop detectors project the
    /// band flux through their onset kernels.
    pub fn observe(
        &mut self,
        spectrum: &Spectrum,
        settings: &DetectorSettings,
        dt: f32,
    ) -> DetectorEvents {
        self.time += dt;
        let mut events = DetectorEvents::default();
        let flux_settings = settings.flux_settings();
        for (band, tracker) in self.bands.iter_mut().enumerate() {
            if let Some(power) =
                tracker.observe(spectrum.band_flux[band], self.time, dt, &flux_settings)
            {
                events.bands[band] = Some(Onset { band, power });
            }
        }
        let snare_flux = kernel_projection(&settings.snare_kernel, &spectrum.band_flux);
        if let Some(snare) = self.snare.observe(
            snare_flux,
            self.time,
            dt,
            &settings.snare_onset_flux_settings(),
        ) {
            events.snare = Some(snare);
            self.kick.choke(self.time, settings.snare_choke);
        }
        if let Some(drop) = self.drop.observe(self.time, spectrum, settings, dt) {
            events.drop = Some(drop);
            self.kick.choke(self.time, settings.kick_drop_choke);
        }
        let kick_flux = kernel_projection(&settings.kick_kernel, &spectrum.band_flux);
        events.kick = self.kick.observe(
            kick_flux,
            self.time,
            dt,
            &settings.kick_onset_flux_settings(),
        );
        events
    }

    pub fn reset(&mut self) {
        for band in &mut self.bands {
            band.reset();
        }
        self.kick.reset();
        self.snare.reset();
        self.drop.reset();
        self.time = 0.0;
    }

    pub fn status(&self) -> DetectionStatus {
        DetectionStatus {
            bands: std::array::from_fn(|index| BandStatus {
                raw_flux: self.bands[index].raw,
                smoothed: self.bands[index].smoothed,
                threshold: self.bands[index].threshold,
                triggers: self.bands[index].triggers,
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
            drop: TriggerStatus {
                flash: self.drop.flash,
                power: self.drop.last_power,
                count: 0,
                armed: self.drop.eligible,
                choked: false,
            },
            kick_flux: OnsetFluxStatus {
                flux: self.kick.onset.raw,
                smoothed: self.kick.onset.smoothed,
                threshold: self.kick.onset.threshold,
            },
            snare_flux: OnsetFluxStatus {
                flux: self.snare.onset.raw,
                smoothed: self.snare.onset.smoothed,
                threshold: self.snare.onset.threshold,
            },
            drop_status: DropStatus {
                low: self.drop.low,
                high: self.drop.high,
                diff: self.drop.diff,
                confidence: self.drop.confidence,
                onset_flux: self.drop.onset.raw,
                onset_smoothed: self.drop.onset.smoothed,
                onset_threshold: self.drop.onset.threshold,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BandStatus {
    pub raw_flux: f32,
    pub smoothed: f32,
    pub threshold: f32,
    /// Number of confirmed flux peaks since the last detector reset.
    pub triggers: u32,
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

/// Drop state behind the trigger: the two projections, their difference, the
/// scaled rolling-window confidence, and the composite onset tracker.
#[derive(Clone, Copy, Debug, Default)]
pub struct DropStatus {
    pub low: f32,
    pub high: f32,
    pub diff: f32,
    pub confidence: f32,
    pub onset_flux: f32,
    pub onset_smoothed: f32,
    pub onset_threshold: f32,
}

/// Raw, smoothed, and adaptive-threshold values from one composite onset
/// tracker.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnsetFluxStatus {
    pub flux: f32,
    pub smoothed: f32,
    pub threshold: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DetectionStatus {
    pub bands: [BandStatus; BAND_COUNT],
    pub kick: TriggerStatus,
    pub snare: TriggerStatus,
    pub drop: TriggerStatus,
    pub kick_flux: OnsetFluxStatus,
    pub snare_flux: OnsetFluxStatus,
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

/// A damped spring used by the flip and quake figure actions.
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

    #[test]
    fn flux_defaults_match_the_onset_recipe() {
        let settings = defaults();
        assert_eq!(settings.flux_attack, 0.005);
        assert_eq!(settings.flux_release, 0.05);
        assert_eq!(settings.flux_average, 0.2);
        assert_eq!(settings.flux_min_interval, 0.03);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn a_flux_peak_triggers_after_the_local_maximum() {
        let settings = defaults().flux_settings();
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
        assert_eq!(tracker.triggers, 1);
    }

    #[test]
    fn a_steady_flux_does_not_trigger() {
        let settings = defaults().flux_settings();
        let mut tracker = FluxTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..120 {
            now += dt;
            assert!(tracker.observe(0.5, now, dt, &settings).is_none());
        }
        assert_eq!(tracker.triggers, 0);
    }

    #[test]
    fn repeated_flux_peaks_respect_the_minimum_interval() {
        let mut flux_settings = defaults().flux_settings();
        flux_settings.min_interval = 0.05;
        let mut tracker = FluxTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for flux in [0.0, 0.0, 1.0, 0.0, 1.0, 0.0] {
            now += dt;
            tracker.observe(flux, now, dt, &flux_settings);
        }
        assert_eq!(
            tracker.triggers, 1,
            "the second pulse lands inside the minimum interval"
        );
    }

    #[test]
    fn the_default_kernels_select_their_bands() {
        let settings = defaults();
        assert!(settings.kick_contains(0));
        assert!(settings.kick_contains(4));
        assert!(!settings.kick_contains(5));
        assert!(settings.snare_contains(5));
        assert!(settings.snare_contains(12));
        assert!(!settings.snare_contains(4));
    }

    #[test]
    fn a_kick_kernel_onset_fires_after_the_flux_peak() {
        let settings = defaults();
        let flux = settings.kick_onset_flux_settings();
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
            band_flux: [0.0; BAND_COUNT],
            ..Spectrum::default()
        };
        for _ in 0..8 {
            bank.observe(&quiet, &settings, dt);
        }
        let pulse = Spectrum {
            bands: [0.5; BAND_COUNT],
            band_flux: [1.0; BAND_COUNT],
            ..Spectrum::default()
        };
        let events = bank.observe(&pulse, &settings, dt);
        assert!(events.snare.is_none());
        let events = bank.observe(&quiet, &settings, dt);
        assert!(events.snare.is_some(), "snare onset");
        assert!(events.kick.is_none(), "the snare chokes the kick");
        assert!(bank.status().kick.choked);
    }

    fn gap_settings() -> DetectorSettings {
        DetectorSettings {
            drop_gap_window: 0.1,
            drop_gap_threshold: 0.1,
            ..DetectorSettings::default()
        }
    }

    fn drop_spectrum(low: f32, high: f32, flux: [f32; BAND_COUNT]) -> Spectrum {
        Spectrum {
            bands: std::array::from_fn(|band| if band < 4 { low } else { high }),
            band_flux: flux,
            ..Spectrum::default()
        }
    }

    fn low_flux(value: f32) -> [f32; BAND_COUNT] {
        std::array::from_fn(|band| if band < 4 { value } else { 0.0 })
    }

    #[test]
    fn the_drop_needs_a_full_gap_window_and_a_composite_onset() {
        let settings = gap_settings();
        let mut drop = DropTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..12 {
            now += dt;
            assert_eq!(
                drop.observe(
                    now,
                    &drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]),
                    &settings,
                    dt
                ),
                None
            );
        }
        assert!(drop.eligible, "the full window is confident");
        assert!(drop.confidence >= settings.drop_gap_threshold);
        assert!((drop.low - 0.0).abs() < 1e-6);
        assert!((drop.high - 0.8).abs() < 1e-6);

        now += dt;
        assert_eq!(
            drop.observe(now, &drop_spectrum(0.0, 0.8, low_flux(1.0)), &settings, dt),
            None,
            "the onset is confirmed on the release hop"
        );
        now += dt;
        let power = drop
            .observe(
                now,
                &drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]),
                &settings,
                dt,
            )
            .expect("the composite onset confirms the drop");
        assert!(power > 0.0);
        assert!(!drop.eligible);
        assert_eq!(drop.confidence, 0.0, "the window restarts after a drop");
    }

    #[test]
    fn eligibility_latches_until_an_onset_resets_it() {
        let settings = gap_settings();
        let mut drop = DropTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..12 {
            now += dt;
            drop.observe(
                now,
                &drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]),
                &settings,
                dt,
            );
        }
        assert!(drop.eligible);

        // The bass returns and confidence falls, but no onset has cleared the
        // latched eligibility yet.
        for _ in 0..20 {
            now += dt;
            drop.observe(
                now,
                &drop_spectrum(0.6, 0.6, [0.0; BAND_COUNT]),
                &settings,
                dt,
            );
        }
        assert!(drop.confidence < settings.drop_gap_threshold);
        assert!(drop.eligible, "the high state stays latched");

        now += dt;
        drop.observe(now, &drop_spectrum(0.6, 0.6, low_flux(1.0)), &settings, dt);
        now += dt;
        assert!(drop
            .observe(
                now,
                &drop_spectrum(0.6, 0.6, [0.0; BAND_COUNT]),
                &settings,
                dt,
            )
            .is_some());
        assert!(!drop.eligible, "the confirming onset clears the latch");
        assert_eq!(drop.confidence, 0.0);
    }

    #[test]
    fn the_confidence_gain_scales_the_window_mean_around_zero() {
        let mut settings = gap_settings();
        settings.drop_gap_threshold = 0.15;
        for (gain, eligible) in [(2.0, true), (0.5, false)] {
            settings.drop_gap_gain = gain;
            let mut drop = DropTracker::default();
            let dt = 1.0 / 86.0;
            let mut now = 0.0;
            for _ in 0..12 {
                now += dt;
                drop.observe(
                    now,
                    &drop_spectrum(0.4, 0.5, [0.0; BAND_COUNT]),
                    &settings,
                    dt,
                );
            }
            assert!((drop.confidence - 0.1 * gain).abs() < 1e-5);
            assert_eq!(drop.eligible, eligible, "gain {gain}");
        }
    }

    #[test]
    fn the_composite_onset_uses_its_own_flux_settings() {
        let mut settings = gap_settings();
        settings.drop_onset_min_threshold = 2.0;
        let mut drop = DropTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        let quiet = drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]);
        for _ in 0..12 {
            now += dt;
            drop.observe(now, &quiet, &settings, dt);
        }
        assert!(drop.eligible);
        now += dt;
        drop.observe(now, &drop_spectrum(0.0, 0.8, low_flux(1.0)), &settings, dt);
        now += dt;
        assert_eq!(
            drop.observe(now, &quiet, &settings, dt),
            None,
            "the dedicated minimum threshold blocks the composite onset"
        );

        settings.drop_onset_min_threshold = 0.01;
        for _ in 0..40 {
            now += dt;
            drop.observe(now, &quiet, &settings, dt);
        }
        now += dt;
        drop.observe(now, &drop_spectrum(0.0, 0.8, low_flux(1.0)), &settings, dt);
        now += dt;
        assert!(drop.observe(now, &quiet, &settings, dt).is_some());
    }

    #[test]
    fn a_partial_window_cannot_confirm_a_drop() {
        let mut settings = gap_settings();
        settings.drop_gap_window = 0.5;
        let mut drop = DropTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..12 {
            now += dt;
            drop.observe(
                now,
                &drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]),
                &settings,
                dt,
            );
        }
        assert!(!drop.eligible, "0.14 s cannot fill a 0.5 s window");

        now += dt;
        drop.observe(now, &drop_spectrum(0.0, 0.8, low_flux(1.0)), &settings, dt);
        now += dt;
        assert_eq!(
            drop.observe(
                now,
                &drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]),
                &settings,
                dt
            ),
            None,
            "an onset inside a partial window is ignored"
        );

        for _ in 0..40 {
            now += dt;
            drop.observe(
                now,
                &drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]),
                &settings,
                dt,
            );
        }
        assert!(drop.eligible, "the window is now full");
        now += dt;
        drop.observe(now, &drop_spectrum(0.0, 0.8, low_flux(1.0)), &settings, dt);
        now += dt;
        assert!(drop
            .observe(
                now,
                &drop_spectrum(0.0, 0.8, [0.0; BAND_COUNT]),
                &settings,
                dt
            )
            .is_some());
    }

    #[test]
    fn a_negative_gap_threshold_accepts_a_negative_confidence() {
        let mut settings = gap_settings();
        settings.drop_gap_threshold = -0.2;
        let mut drop = DropTracker::default();
        let dt = 1.0 / 86.0;
        let mut now = 0.0;
        for _ in 0..12 {
            now += dt;
            drop.observe(
                now,
                &drop_spectrum(0.6, 0.5, [0.0; BAND_COUNT]),
                &settings,
                dt,
            );
        }
        assert!((drop.confidence + 0.1).abs() < 1e-5);
        assert!(
            drop.eligible,
            "a negative confidence can still clear a negative threshold"
        );
    }

    #[test]
    fn kernel_projection_is_a_normalized_weighted_mean() {
        let mut kernels = DropKernels {
            low: [0.0; BAND_COUNT],
            ..DropKernels::default()
        };
        kernels.set(DropKernel::Low, 0, 0.25);
        kernels.set(DropKernel::Low, 1, 1.0);
        let mut values = [0.0; BAND_COUNT];
        values[0] = 0.2;
        values[1] = 0.8;
        let expected = (0.25 * 0.2 + 1.0 * 0.8) / 1.25;
        assert!((kernels.projection(DropKernel::Low, &values) - expected).abs() < 1e-6);

        let empty = DropKernels {
            low: [0.0; BAND_COUNT],
            ..DropKernels::default()
        };
        assert_eq!(empty.projection(DropKernel::Low, &values), 0.0);
        kernels.set(DropKernel::Low, 2, 2.0);
        assert_eq!(kernels.low[2], 1.0, "kernel weights stay inside 0..1");
    }

    #[test]
    fn the_gap_window_mean_trades_depth_for_duration() {
        let mut window = GapWindow::with_capacity(256);
        for index in 0..100 {
            let at = index as f32 * 0.01;
            let value = if index < 25 { 0.8 } else { 0.0 };
            window.push(at, value, 1.0);
        }
        assert!((window.mean() - 0.2).abs() < 1e-5);
        window.push(1.5, 0.0, 1.0);
        assert!(
            window.samples.front().is_some_and(|&(at, _)| at >= 0.5),
            "samples older than the window are pruned"
        );
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
        settings.flux_attack = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.kick_kernel[0] = -0.1;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.snare_kernel[0] = f32::NAN;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.kick_onset_attack = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.snare_onset_sensitivity = 9.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_window = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_window = 5.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_threshold = 1.5;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_kernels.low[0] = -0.1;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_kernels.high[0] = f32::NAN;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_gain = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_gap_gain = 20.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.drop_onset_attack = 0.0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.kick_sequence_every = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn the_onset_power_is_the_smoothed_flux_at_the_peak() {
        let settings = defaults().flux_settings();
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
