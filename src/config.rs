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
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -6.0,
            Adjustment::Increase => 6.0,
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: crate::theme::ThemeId,
    pub preamp_db: PreampDb,
    pub frame_rate_limit: FrameRateLimit,
    pub shutter_frames: ShutterFrames,
}

pub fn config_file() -> anyhow::Result<PathBuf> {
    let directories = ProjectDirs::from("com", "zezic", "vu")
        .context("Could not resolve VU's user config directory")?;
    Ok(directories.config_dir().join("vu-config.ron"))
}
