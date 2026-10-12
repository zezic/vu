use serde::{Deserialize, Serialize};

use crate::{db_to_normalized, scales::Scale};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Vu,
    Ppm,
    Rms,
    SamplePeak,
}

impl Mode {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Vu => Self::Ppm,
            Self::Ppm => Self::Rms,
            Self::Rms => Self::SamplePeak,
            Self::SamplePeak => Self::Vu,
        }
    }

    pub fn uses_peak_movement(self) -> bool {
        matches!(self, Self::Ppm | Self::SamplePeak)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Vu => "VU",
            Self::Ppm => "PPM (BBC)",
            Self::Rms => "RMS 300 ms",
            Self::SamplePeak => "Sample Peak",
        }
    }

    pub fn units(self) -> &'static str {
        match self {
            Self::Vu => "VU",
            Self::Ppm | Self::Rms | Self::SamplePeak => "dBFS",
        }
    }

    /// VU deflection is proportional to rectified drive, not its logarithm.
    pub fn position(self, scale: &Scale, level: f32) -> f32 {
        match self {
            Self::Vu => level / db_to_normalized(6.0),
            Self::Ppm | Self::Rms | Self::SamplePeak => scale.position(level),
        }
    }
}
