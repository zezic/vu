use std::path::PathBuf;

use anyhow::{ensure, Context};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{
    db_to_multiplier,
    frame_schedule::{Adjustment, FrameRateLimit},
};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct PreampDb(f32);

#[derive(Clone, Copy)]
pub enum PreampStep {
    Coarse,
    Fine,
}

impl Default for PreampDb {
    fn default() -> Self {
        Self(0.0)
    }
}

impl TryFrom<f32> for PreampDb {
    type Error = anyhow::Error;

    fn try_from(db: f32) -> Result<Self, Self::Error> {
        ensure!(
            db.is_finite() && (-96.0..=96.0).contains(&db),
            "preamp_db must be finite and between -96 and 96 dB"
        );
        Ok(Self(db))
    }
}

impl From<PreampDb> for f32 {
    fn from(value: PreampDb) -> Self {
        value.0
    }
}

impl PreampDb {
    pub fn db(self) -> f32 {
        self.0
    }

    pub fn gain(self) -> f32 {
        db_to_multiplier(self.0)
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment, step: PreampStep) -> Self {
        let magnitude = match step {
            PreampStep::Coarse => 6.0,
            PreampStep::Fine => 1.0,
        };
        let delta = match adjustment {
            Adjustment::Decrease => -magnitude,
            Adjustment::Increase => magnitude,
        };
        Self((self.0 + delta).clamp(-96.0, 96.0))
    }
}

/// Camera magnification, independent of the window size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct Zoom(f64);

impl Zoom {
    pub const MIN: f64 = 0.35;
    pub const MAX: f64 = 3.0;

    pub fn factor(self) -> f64 {
        self.0
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -0.1,
            Adjustment::Increase => 0.1,
        };
        Self((self.0 + delta).clamp(Self::MIN, Self::MAX))
    }

    #[must_use]
    pub fn scaled(self, factor: f64) -> Self {
        assert!(
            factor.is_finite() && factor > 0.0,
            "zoom scaling must be finite and positive"
        );
        Self((self.0 * factor).clamp(Self::MIN, Self::MAX))
    }
}

impl Default for Zoom {
    fn default() -> Self {
        Self(1.0)
    }
}

impl TryFrom<f64> for Zoom {
    type Error = anyhow::Error;

    fn try_from(factor: f64) -> Result<Self, Self::Error> {
        ensure!(
            factor.is_finite() && (Self::MIN..=Self::MAX).contains(&factor),
            "zoom must be finite and between {} and {}",
            Self::MIN,
            Self::MAX
        );
        Ok(Self(factor))
    }
}

impl From<Zoom> for f64 {
    fn from(value: Zoom) -> Self {
        value.0
    }
}

/// Exposure length in frame intervals, bounded to keep motion history finite.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct ShutterFrames(f32);

impl ShutterFrames {
    pub const MAX: f32 = 16.0;

    pub fn frames(self) -> f32 {
        self.0
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -0.5,
            Adjustment::Increase => 0.5,
        };
        Self((self.0 + delta).clamp(1.0, Self::MAX))
    }
}

impl Default for ShutterFrames {
    fn default() -> Self {
        Self(2.0)
    }
}

impl TryFrom<f32> for ShutterFrames {
    type Error = anyhow::Error;

    fn try_from(frames: f32) -> Result<Self, Self::Error> {
        ensure!(
            frames.is_finite() && (1.0..=Self::MAX).contains(&frames),
            "shutter_frames must be finite and between 1 and 16 frames"
        );
        Ok(Self(frames))
    }
}

impl From<ShutterFrames> for f32 {
    fn from(value: ShutterFrames) -> Self {
        value.0
    }
}

const VISUALIZER_TEXT_MIN: f32 = 0.6;
const VISUALIZER_TEXT_MAX: f32 = 2.0;
const VISUALIZER_TEXT_STEP: f32 = 0.1;

/// Visualizer chrome text multiplier, applied on top of the display DPI scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct VisualizerTextScale(f32);

impl Default for VisualizerTextScale {
    fn default() -> Self {
        Self(1.2)
    }
}

impl TryFrom<f32> for VisualizerTextScale {
    type Error = anyhow::Error;

    fn try_from(scale: f32) -> Result<Self, Self::Error> {
        ensure!(
            scale.is_finite() && (VISUALIZER_TEXT_MIN..=VISUALIZER_TEXT_MAX).contains(&scale),
            "visualizer_text_scale must be finite and between {VISUALIZER_TEXT_MIN} and {VISUALIZER_TEXT_MAX}"
        );
        Ok(Self(scale))
    }
}

impl From<VisualizerTextScale> for f32 {
    fn from(value: VisualizerTextScale) -> Self {
        value.0
    }
}

impl VisualizerTextScale {
    pub fn factor(self) -> f32 {
        self.0
    }

    pub fn percent(self) -> u32 {
        (self.0 * 100.0).round() as u32
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -VISUALIZER_TEXT_STEP,
            Adjustment::Increase => VISUALIZER_TEXT_STEP,
        };
        Self((self.0 + delta).clamp(VISUALIZER_TEXT_MIN, VISUALIZER_TEXT_MAX))
    }
}

const PRESENTATION_OPACITY_MIN: f32 = 0.0;
const PRESENTATION_OPACITY_MAX: f32 = 2.0;
const PRESENTATION_OPACITY_STEP: f32 = 0.05;

/// Multiplier applied to the presentation text and infographic layer before
/// additive or multiplicative composition.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct PresentationOpacity(f32);

impl Default for PresentationOpacity {
    fn default() -> Self {
        Self(1.0)
    }
}

impl TryFrom<f32> for PresentationOpacity {
    type Error = anyhow::Error;

    fn try_from(opacity: f32) -> Result<Self, Self::Error> {
        ensure!(
            opacity.is_finite()
                && (PRESENTATION_OPACITY_MIN..=PRESENTATION_OPACITY_MAX).contains(&opacity),
            "presentation_opacity must be finite and between {PRESENTATION_OPACITY_MIN} and {PRESENTATION_OPACITY_MAX}"
        );
        Ok(Self(opacity))
    }
}

impl From<PresentationOpacity> for f32 {
    fn from(value: PresentationOpacity) -> Self {
        value.0
    }
}

impl PresentationOpacity {
    pub fn factor(self) -> f32 {
        self.0
    }

    pub fn percent(self) -> u32 {
        (self.0 * 100.0).round() as u32
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -PRESENTATION_OPACITY_STEP,
            Adjustment::Increase => PRESENTATION_OPACITY_STEP,
        };
        Self((self.0 + delta).clamp(PRESENTATION_OPACITY_MIN, PRESENTATION_OPACITY_MAX))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorVisibility {
    Always,
    #[default]
    AutoHide,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: crate::theme::ThemeId,
    pub theme_preferences: crate::theme::ThemePreferences,
    pub fullscreen: bool,
    pub cursor_visibility: CursorVisibility,
    pub preamp_db: PreampDb,
    pub zoom: Zoom,
    pub frame_rate_limit: FrameRateLimit,
    pub shutter_frames: ShutterFrames,
    pub visualizer_text_scale: VisualizerTextScale,
    pub presentation_opacity: PresentationOpacity,
    pub detectors: crate::visualizer::DetectorSettings,
    pub depth_fade: crate::visualizer::DepthFade,
    pub near_opacity: crate::visualizer::NearOpacity,
    pub crease_threshold: crate::visualizer::CreaseThreshold,
    pub line_width: crate::visualizer::LineWidth,
    pub model_motion: crate::visualizer::ModelMotion,
    pub model_rotation: crate::visualizer::ModelRotation,
    pub motion_opacity: crate::visualizer::MotionOpacity,
    pub motion_fade_power: crate::visualizer::MotionFadePower,
    pub tuning: crate::visualizer::VisualizerTuning,
    /// Removed display selectors accepted so an existing config still loads.
    #[serde(default, skip_serializing)]
    presentation: Ignored,
    #[serde(default, skip_serializing)]
    mode: Ignored,
    #[serde(default, skip_serializing)]
    peak_movement: Ignored,
}

/// A config value that is accepted for compatibility and discarded.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Ignored;

impl<'de> Deserialize<'de> for Ignored {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(Self)
    }
}

pub fn config_file() -> anyhow::Result<PathBuf> {
    let directories = ProjectDirs::from("com", "zezic", "vu")
        .context("Could not resolve VU's user config directory")?;
    Ok(directories.config_dir().join("vu-config.ron"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_preserves_camera_bounds_across_dials_and_wheel_scaling() {
        let normal = Zoom::default();
        assert_eq!(normal.factor(), 1.0);
        assert_eq!(normal.adjusted(Adjustment::Increase).factor(), 1.1);
        assert_eq!(normal.scaled(2.0).factor(), 2.0);
        assert_eq!(normal.scaled(0.01).factor(), Zoom::MIN);
        assert_eq!(normal.scaled(100.0).factor(), Zoom::MAX);
        assert_eq!(
            Zoom::try_from(Zoom::MIN)
                .unwrap()
                .adjusted(Adjustment::Decrease)
                .factor(),
            Zoom::MIN
        );
        assert_eq!(
            Zoom::try_from(Zoom::MAX)
                .unwrap()
                .adjusted(Adjustment::Increase)
                .factor(),
            Zoom::MAX
        );
        for invalid in [0.34, 3.01, f64::NAN, f64::INFINITY] {
            assert!(Zoom::try_from(invalid).is_err());
        }
    }

    #[test]
    fn presentation_opacity_steps_between_sane_bounds() {
        let normal = PresentationOpacity::default();
        assert_eq!(normal.percent(), 100);
        assert_eq!(normal.adjusted(Adjustment::Increase).percent(), 105);
        assert_eq!(normal.adjusted(Adjustment::Decrease).percent(), 95);
        let mut value = normal;
        for _ in 0..100 {
            value = value.adjusted(Adjustment::Increase);
        }
        assert_eq!(value.percent(), 200);
        for _ in 0..100 {
            value = value.adjusted(Adjustment::Decrease);
        }
        assert_eq!(value.percent(), 0);
        assert!(PresentationOpacity::try_from(2.01).is_err());
        assert!(PresentationOpacity::try_from(-0.01).is_err());
        assert!(PresentationOpacity::try_from(f32::NAN).is_err());
    }

    #[test]
    fn visualizer_text_scale_steps_between_sane_bounds() {
        let normal = VisualizerTextScale::default();
        assert_eq!(normal.percent(), 120);
        assert_eq!(normal.adjusted(Adjustment::Increase).percent(), 130);
        assert_eq!(normal.adjusted(Adjustment::Decrease).percent(), 110);

        let mut smallest = normal;
        for _ in 0..20 {
            smallest = smallest.adjusted(Adjustment::Decrease);
        }
        assert_eq!(smallest.percent(), 60);

        let mut largest = normal;
        for _ in 0..20 {
            largest = largest.adjusted(Adjustment::Increase);
        }
        assert_eq!(largest.percent(), 200);

        assert!(VisualizerTextScale::try_from(0.5).is_err());
        assert!(VisualizerTextScale::try_from(2.1).is_err());
        assert!(VisualizerTextScale::try_from(f32::NAN).is_err());
    }
}
