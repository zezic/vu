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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub preamp_db: PreampDb,
    pub frame_rate_limit: FrameRateLimit,
}

pub fn config_file() -> anyhow::Result<PathBuf> {
    let directories = ProjectDirs::from("com", "zezic", "vu")
        .context("Could not resolve VU's user config directory")?;
    Ok(directories.config_dir().join("vu-config.ron"))
}
