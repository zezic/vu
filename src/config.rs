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
        Self(18.0)
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
        Self(1.0)
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub presentation: crate::visualizer::Presentation,
    pub mode: crate::measurement::Mode,
    pub peak_movement: crate::meter::PeakMovement,
    pub theme: crate::theme::ThemeId,
    pub preamp_db: PreampDb,
    pub frame_rate_limit: FrameRateLimit,
    pub shutter_frames: ShutterFrames,
    pub visualizer_text_scale: VisualizerTextScale,
    pub detectors: crate::visualizer::DetectorSettings,
    pub depth_fade: crate::visualizer::DepthFade,
    pub near_opacity: crate::visualizer::NearOpacity,
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
    fn visualizer_text_scale_steps_between_sane_bounds() {
        let normal = VisualizerTextScale::default();
        assert_eq!(normal.percent(), 100);
        assert_eq!(normal.adjusted(Adjustment::Increase).percent(), 110);
        assert_eq!(normal.adjusted(Adjustment::Decrease).percent(), 90);

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
